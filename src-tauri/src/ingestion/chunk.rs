
/// Configuration for the recursive character splitter.
const TARGET_TOKENS: usize = 800;
const MAX_TOKENS: usize = 1500;
const OVERLAP_TOKENS: usize = 100;
const MIN_CHUNK_TOKENS: usize = 50;

/// Approximate tokens by dividing character count by 4.
fn approx_tokens(text: &str) -> usize {
    text.len() / 4
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
/// Respects section headings (markdown), paragraph breaks, line breaks, sentences, word boundaries.
pub fn chunk_text(text: &str, source_id: &str) -> Vec<ChunkData> {
    if text.is_empty() {
        return Vec::new();
    }

    let target_chars = TARGET_TOKENS * 4;
    let max_chars = MAX_TOKENS * 4;
    let overlap_chars = OVERLAP_TOKENS * 4;
    let min_chars = MIN_CHUNK_TOKENS * 4;

    let raw_chunks = recursive_split(text, target_chars, max_chars, overlap_chars);

    let mut result = Vec::new();
    let mut current_offset = 0;

    for chunk_text in &raw_chunks {
        let trimmed = chunk_text.trim();
        if trimmed.len() < min_chars && !raw_chunks.is_empty() && raw_chunks.len() > 1 {
            continue;
        }

        let start = text[current_offset..]
            .find(trimmed)
            .map(|pos| current_offset + pos)
            .unwrap_or(current_offset);
        let end = start + trimmed.len();

        result.push(ChunkData {
            id: format!("{}-c{}", source_id, result.len()),
            chunk_index: result.len() as i32,
            content: trimmed.to_string(),
            token_count: Some(approx_tokens(trimmed) as i32),
            start_offset: Some(start as i32),
            end_offset: Some(end as i32),
            metadata: None,
        });

        // Move offset forward (accounting for overlap)
        if end > overlap_chars {
            current_offset = end.saturating_sub(overlap_chars);
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

/// Recursively split text, respecting boundaries in priority order:
/// section headings > paragraph breaks > line breaks > sentence ends > word boundaries
fn recursive_split(text: &str, target: usize, max: usize, overlap: usize) -> Vec<String> {
    if text.len() <= max {
        return vec![text.to_string()];
    }

    // Try splitting by section headings (markdown ## headings)
    let separators = [
        "\n## ",      // Section heading
        "\n### ",     // Subsection
        "\n\n",       // Paragraph break
        "\n",         // Line break
        ". ",         // Sentence end
        " ",          // Word boundary
    ];

    for sep in &separators {
        let parts: Vec<&str> = text.split(sep).collect();
        if parts.len() > 1 {
            let mut chunks = Vec::new();
            let mut current = String::new();

            for (i, part) in parts.iter().enumerate() {
                let with_sep = if i > 0 {
                    format!("{}{}", sep, part)
                } else {
                    part.to_string()
                };

                if current.len() + with_sep.len() > target && !current.is_empty() {
                    chunks.push(current.clone());
                    // Start new chunk with overlap
                    let overlap_start = current.len().saturating_sub(overlap);
                    current = current[overlap_start..].to_string();
                    current.push_str(&with_sep);
                } else {
                    current.push_str(&with_sep);
                }
            }

            if !current.is_empty() {
                chunks.push(current);
            }

            // Recursively split any chunks that are still too large
            let mut result = Vec::new();
            for chunk in chunks {
                if chunk.len() > max {
                    result.extend(recursive_split(&chunk, target, max, overlap));
                } else {
                    result.push(chunk);
                }
            }

            return result;
        }
    }

    // Fallback: hard split at max chars
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + target).min(text.len());
        // Try to find a word boundary near the target
        let chunk_end = if end < text.len() {
            text[start..end]
                .rfind(' ')
                .map(|pos| start + pos + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..chunk_end].to_string());
        start = chunk_end.saturating_sub(overlap);
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
}
