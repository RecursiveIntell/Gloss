/// Configuration for the recursive character splitter.
const TARGET_TOKENS: usize = 800;
const MAX_TOKENS: usize = 1500;
const OVERLAP_TOKENS: usize = 100;
const MIN_CHUNK_TOKENS: usize = 50;

/// Approximate tokens by dividing character count by 4.
fn approx_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Snap a byte offset to the nearest valid UTF-8 char boundary.
/// Searches backward from `offset` to find a valid boundary.
fn snap_to_char_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut pos = offset;
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// A chunk produced by the splitter.
pub struct ChunkData {
    pub id: String,
    pub chunk_index: i32,
    pub content: String,
    pub token_count: Option<i32>,
    pub start_offset: Option<i32>,
    pub end_offset: Option<i32>,
    pub metadata: Option<String>,
}

/// Split text into chunks using recursive character splitting.
/// When `source_title` has a recognized code extension, uses code-aware
/// boundary detection to prefer splitting at structural boundaries.
pub fn chunk_text(text: &str, source_id: &str) -> Vec<ChunkData> {
    chunk_text_with_title(text, source_id, "")
}

/// Split text into chunks, using the source title for code-aware splitting.
pub fn chunk_text_with_title(text: &str, source_id: &str, source_title: &str) -> Vec<ChunkData> {
    if text.is_empty() {
        return Vec::new();
    }

    let target_chars = TARGET_TOKENS * 4;
    let max_chars = MAX_TOKENS * 4;
    let overlap_chars = OVERLAP_TOKENS * 4;
    let min_chars = MIN_CHUNK_TOKENS * 4;

    // Try code-aware splitting first
    let ext = source_title.rsplit('.').next().unwrap_or("");
    let structural_boundaries = get_code_boundaries(ext, text);

    let raw_chunks = if !structural_boundaries.is_empty() {
        code_aware_split(
            text,
            &structural_boundaries,
            target_chars,
            max_chars,
            overlap_chars,
        )
    } else {
        recursive_split(text, target_chars, max_chars, overlap_chars)
    };

    let mut result = Vec::new();
    let mut current_offset = 0;

    for chunk_text in &raw_chunks {
        let trimmed = chunk_text.trim();
        if trimmed.len() < min_chars && !raw_chunks.is_empty() && raw_chunks.len() > 1 {
            continue;
        }

        let safe_offset = snap_to_char_boundary(text, current_offset);
        let start = text[safe_offset..]
            .find(trimmed)
            .map(|pos| safe_offset + pos)
            .unwrap_or(safe_offset);
        let end = start + trimmed.len();

        // Extract heading/section info for chunk metadata
        let section = extract_section_heading(trimmed, ext);
        let metadata = section.map(|s| serde_json::json!({ "section": s }).to_string());

        result.push(ChunkData {
            id: format!("{}-c{}", source_id, result.len()),
            chunk_index: result.len() as i32,
            content: trimmed.to_string(),
            token_count: Some(approx_tokens(trimmed) as i32),
            start_offset: Some(start as i32),
            end_offset: Some(end as i32),
            metadata,
        });

        // Move offset forward (accounting for overlap)
        if end > overlap_chars {
            current_offset = snap_to_char_boundary(text, end.saturating_sub(overlap_chars));
        }
    }

    // If no chunks were produced but text exists, produce one chunk
    if result.is_empty() && !text.trim().is_empty() {
        result.push(ChunkData {
            id: format!("{}-c0", source_id),
            chunk_index: 0,
            content: text.trim().to_string(),
            token_count: Some(approx_tokens(text.trim()) as i32),
            start_offset: Some(0),
            end_offset: Some(text.len() as i32),
            metadata: None,
        });
    }

    result
}

/// Detect structural boundaries in code files based on language.
fn get_code_boundaries(ext: &str, content: &str) -> Vec<usize> {
    match ext {
        "rs" => find_rust_boundaries(content),
        "ts" | "tsx" | "js" | "jsx" => find_typescript_boundaries(content),
        "py" => find_python_boundaries(content),
        _ => Vec::new(),
    }
}

fn find_rust_boundaries(content: &str) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut offset = 0;
    let bytes = content.as_bytes();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("pub fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("impl ")
            || trimmed.starts_with("pub struct ")
            || trimmed.starts_with("pub enum ")
            || trimmed.starts_with("pub trait ")
            || trimmed.starts_with("pub mod ")
            || trimmed.starts_with("mod ")
            || trimmed.starts_with("#[cfg(test)]")
        {
            boundaries.push(offset);
        }
        offset += line.len();
        // Skip past actual line ending (\r\n or \n)
        if bytes.get(offset) == Some(&b'\r') {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b'\n') {
            offset += 1;
        }
    }

    boundaries
}

fn find_typescript_boundaries(content: &str) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut offset = 0;
    let bytes = content.as_bytes();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("export function ")
            || trimmed.starts_with("export async function ")
            || trimmed.starts_with("export const ")
            || trimmed.starts_with("export default ")
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("export type ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("export enum ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("function ")
            || trimmed.starts_with("// ===")
        {
            boundaries.push(offset);
        }
        offset += line.len();
        if bytes.get(offset) == Some(&b'\r') {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b'\n') {
            offset += 1;
        }
    }

    boundaries
}

fn find_python_boundaries(content: &str) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut offset = 0;
    let bytes = content.as_bytes();

    for line in content.lines() {
        let trimmed = line.trim();
        let is_top_level = !line.starts_with(' ') && !line.starts_with('\t');
        if is_top_level
            && (trimmed.starts_with("def ")
                || trimmed.starts_with("async def ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("# ===")
                || trimmed.starts_with('@'))
        {
            boundaries.push(offset);
        }
        offset += line.len();
        if bytes.get(offset) == Some(&b'\r') {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b'\n') {
            offset += 1;
        }
    }

    boundaries
}

/// Split content at structural boundaries, merging small segments and
/// recursively splitting large ones.
fn code_aware_split(
    content: &str,
    boundaries: &[usize],
    target: usize,
    max: usize,
    overlap: usize,
) -> Vec<String> {
    // Split content into segments at boundaries
    let mut segments: Vec<&str> = Vec::new();
    let mut prev = 0;
    for &boundary in boundaries {
        if boundary > prev && boundary <= content.len() {
            segments.push(&content[prev..boundary]);
            prev = boundary;
        }
    }
    if prev < content.len() {
        segments.push(&content[prev..]);
    }

    // Merge small segments and split large ones
    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in segments {
        if current.is_empty() {
            current = segment.to_string();
        } else if current.len() + segment.len() <= target {
            // Merge small segments
            current.push_str(segment);
        } else {
            // Current is big enough, push it
            if current.len() >= target / 4 {
                chunks.push(current.clone());
            } else {
                // Too small on its own, merge with next
                current.push_str(segment);
                if current.len() >= target / 2 {
                    chunks.push(current.clone());
                    current = String::new();
                }
                continue;
            }
            current = segment.to_string();
        }

        // If current segment itself exceeds max, recursively split it
        if current.len() > max {
            let sub = recursive_split(&current, target, max, overlap);
            chunks.extend(sub);
            current = String::new();
        }
    }

    // Push remaining
    if !current.is_empty() {
        if current.len() > max {
            chunks.extend(recursive_split(&current, target, max, overlap));
        } else {
            chunks.push(current);
        }
    }

    chunks
}

/// Extract a heading from the first meaningful line of a chunk for metadata.
fn extract_section_heading(chunk: &str, ext: &str) -> Option<String> {
    let first_line = chunk.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = first_line.trim();

    match ext {
        "rs" => {
            if trimmed.starts_with("impl ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("fn ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("pub trait ")
            {
                // Extract up to the opening brace or end of line
                let heading = trimmed.split('{').next().unwrap_or(trimmed).trim();
                Some(heading.to_string())
            } else {
                None
            }
        }
        "ts" | "tsx" | "js" | "jsx" => {
            if trimmed.starts_with("export ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("interface ")
            {
                let heading = trimmed.split('{').next().unwrap_or(trimmed).trim();
                Some(heading.to_string())
            } else {
                None
            }
        }
        "py" => {
            if trimmed.starts_with("def ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("async def ")
            {
                let heading = trimmed.split(':').next().unwrap_or(trimmed).trim();
                Some(heading.to_string())
            } else {
                None
            }
        }
        "md" | "markdown" => {
            if trimmed.starts_with('#') {
                Some(trimmed.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn find_separator_split(remaining: &str, sep: &str, target: usize, max: usize) -> Option<usize> {
    let max_end = snap_to_char_boundary(remaining, remaining.len().min(max));
    if max_end == 0 {
        return None;
    }

    let target_end = snap_to_char_boundary(remaining, remaining.len().min(target).min(max_end));
    let min_chunk = (target / 4).max(1).min(target_end.max(1));

    if let Some(pos) = remaining[..target_end].rfind(sep) {
        if pos >= min_chunk {
            return Some(pos);
        }
    }

    if target_end < max_end {
        let tail = &remaining[target_end..max_end];
        if let Some(offset) = tail.find(sep) {
            let pos = target_end + offset;
            if pos >= min_chunk {
                return Some(pos);
            }
        }
    }

    None
}

fn hard_split_end(remaining: &str, target: usize, max: usize) -> usize {
    let hard_end = snap_to_char_boundary(remaining, remaining.len().min(max));
    if hard_end == 0 {
        return 0;
    }

    let target_end = snap_to_char_boundary(remaining, remaining.len().min(target).min(hard_end));
    if target_end == 0 {
        return hard_end;
    }

    remaining[..target_end]
        .rfind(' ')
        .filter(|pos| *pos > 0)
        .unwrap_or(target_end)
}

/// Split text iteratively, respecting boundaries in priority order:
/// section headings > paragraph breaks > line breaks > sentence ends > word boundaries.
/// This version guarantees forward progress and avoids recursive blow-ups on
/// pathological long files.
fn recursive_split(text: &str, target: usize, max: usize, overlap: usize) -> Vec<String> {
    if text.len() <= max {
        return vec![text.to_string()];
    }

    let separators = [
        "\n## ",  // Section heading
        "\n### ", // Subsection
        "\n\n",   // Paragraph break
        "\n",     // Line break
        ". ",     // Sentence end
        " ",      // Word boundary
    ];

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < text.len() {
        let remaining = &text[start..];
        if remaining.len() <= max {
            chunks.push(remaining.to_string());
            break;
        }

        let mut end_rel = separators
            .iter()
            .find_map(|sep| find_separator_split(remaining, sep, target, max))
            .unwrap_or_else(|| hard_split_end(remaining, target, max));

        end_rel = snap_to_char_boundary(remaining, end_rel);
        if end_rel == 0 {
            end_rel = snap_to_char_boundary(remaining, remaining.len().min(max));
        }

        if end_rel == 0 {
            break;
        }

        let end = start + end_rel;
        if end <= start {
            let fallback_end = snap_to_char_boundary(text, (start + target).min(text.len()));
            if fallback_end <= start {
                break;
            }
            chunks.push(text[start..fallback_end].to_string());
            start = fallback_end;
            continue;
        }

        chunks.push(text[start..end].to_string());

        let new_start = snap_to_char_boundary(text, end.saturating_sub(overlap));
        start = if new_start <= start { end } else { new_start };
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_text_single_chunk() {
        let chunks = chunk_text("Hello world", "s1");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Hello world");
    }

    #[test]
    fn test_empty_text() {
        let chunks = chunk_text("", "s1");
        assert_eq!(chunks.len(), 0);
    }

    #[test]
    fn test_long_text_multiple_chunks() {
        let text = "This is a test. ".repeat(500); // ~8000 chars
        let chunks = chunk_text(&text, "s1");
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.content.len() <= MAX_TOKENS * 4 + 100); // allow some slack
        }
    }

    #[test]
    fn test_repeated_text_chunking_stays_bounded() {
        let text = "token ".repeat(20_000);
        let chunks = chunk_text(&text, "s1");
        assert!(!chunks.is_empty());
        assert!(chunks.len() < 1000);
        assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
    }

    #[test]
    fn test_markdown_heading_split() {
        let text = format!(
            "# Introduction\n\n{}\n\n## Methods\n\n{}\n\n## Results\n\n{}",
            "Content here. ".repeat(200),
            "Method details. ".repeat(200),
            "Results data. ".repeat(200),
        );
        let chunks = chunk_text(&text, "s1");
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_chunk_offsets() {
        let text = "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.";
        let chunks = chunk_text(text, "s1");
        for chunk in &chunks {
            assert!(chunk.start_offset.is_some());
            assert!(chunk.end_offset.is_some());
        }
    }

    #[test]
    fn test_rust_code_boundaries() {
        let code = "use std::io;\n\npub fn foo() {\n    println!(\"hello\");\n}\n\npub fn bar() {\n    println!(\"world\");\n}\n";
        let boundaries = find_rust_boundaries(code);
        assert!(
            boundaries.len() >= 2,
            "Should find at least 2 function boundaries"
        );
    }

    #[test]
    fn test_code_aware_chunking() {
        // Build a Rust file large enough to need splitting
        let mut code = String::from("use std::io;\n\n");
        for i in 0..20 {
            code.push_str(&format!(
                "pub fn function_{}() {{\n{}\n}}\n\n",
                i,
                "    let x = 42;\n".repeat(40)
            ));
        }
        let chunks = chunk_text_with_title(&code, "s1", "example.rs");
        assert!(chunks.len() > 1, "Should split into multiple chunks");
        // Each chunk should tend to start at a function boundary
        for chunk in &chunks {
            if chunk.chunk_index > 0 {
                let first_line = chunk.content.lines().next().unwrap_or("");
                let trimmed = first_line.trim();
                // Many chunks should start at function boundaries
                // (not all, since merging may affect this)
                if trimmed.starts_with("pub fn ") {
                    assert!(
                        chunk.metadata.is_some(),
                        "Code chunks should have section metadata"
                    );
                }
            }
        }
    }

    #[test]
    fn test_section_heading_extraction() {
        assert_eq!(
            extract_section_heading("pub fn execute(&self) {\n    // body\n}", "rs"),
            Some("pub fn execute(&self)".to_string())
        );
        assert_eq!(
            extract_section_heading("export function handleClick() {\n}", "ts"),
            Some("export function handleClick()".to_string())
        );
        assert_eq!(
            extract_section_heading("def process(data):\n    pass", "py"),
            Some("def process(data)".to_string())
        );
    }
}
