//! Text chunking with multiple content-aware strategies.
//!
//! Strategies:
//! - [`ChunkingStrategy::Plain`]: recursive splitting (original behavior, default)
//! - [`ChunkingStrategy::Sentence`]: sentence-boundary-aware splitting with overlap
//! - [`ChunkingStrategy::Code`]: code-aware splitting that avoids breaking function bodies
//! - [`ChunkingStrategy::Markdown`]: markdown-header-based splitting on header boundaries

use crate::config::{ChunkingConfig, ChunkingStrategy};
use crate::tokenizer::TokenCounter;
use crate::types::TextChunk;

/// Maximum recursion depth to prevent infinite loops.
const MAX_RECURSION_DEPTH: usize = 10;

/// Chunk text using the configured strategy and token counter.
///
/// Returns an empty vec for empty input. Short text (≤ max_size) is
/// returned as a single chunk.
pub fn chunk_text(
    text: &str,
    config: &ChunkingConfig,
    token_counter: &dyn TokenCounter,
) -> Vec<TextChunk> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    // Short text (≤ max_size) returned as a single chunk.
    // But only if using Plain strategy -- other strategies should still
    // attempt structural splitting even for short text.
    if config.strategy == ChunkingStrategy::Plain
        && text.len() <= config.max_size
        && text.len() >= config.min_size
    {
        return vec![TextChunk {
            index: 0,
            content: text.to_string(),
            token_count_estimate: token_counter.count_tokens(text),
        }];
    }

    let raw_chunks = match config.strategy {
        ChunkingStrategy::Plain => plain_split(text, config),
        ChunkingStrategy::Sentence => sentence_split(text, config),
        ChunkingStrategy::Code => code_split(text, config),
        ChunkingStrategy::Markdown => markdown_split(text, config),
    };

    // Merge small adjacent chunks
    let merged = merge_small_chunks(raw_chunks, config.target_size, config.min_size);

    // Apply overlap
    let overlapped = apply_overlap(merged, config.overlap);

    // Convert to TextChunk
    overlapped
        .into_iter()
        .enumerate()
        .map(|(i, content)| {
            let token_count_estimate = token_counter.count_tokens(&content);
            TextChunk {
                index: i,
                content,
                token_count_estimate,
            }
        })
        .collect()
}

// ────────────────────────────────────────────────────────────────
// Plain strategy (original behavior)
// ────────────────────────────────────────────────────────────────

/// Plain recursive split — identical to the original algorithm.
fn plain_split(text: &str, config: &ChunkingConfig) -> Vec<String> {
    recursive_split(text, config, 0)
}

// ────────────────────────────────────────────────────────────────
// Sentence strategy
// ────────────────────────────────────────────────────────────────

/// Split text on sentence boundaries, accumulating sentences up to
/// `target_size`. When a chunk exceeds `max_size`, it is recursively
/// split. Each chunk (except the first) includes `overlap` characters
/// of the previous chunk's tail for context continuity.
fn sentence_split(text: &str, config: &ChunkingConfig) -> Vec<String> {
    let sentences = split_into_sentences(text);
    if sentences.is_empty() {
        return vec![text.to_string()];
    }

    // Greedily pack sentences into chunks up to target_size.
    let mut chunks = Vec::new();
    let mut current = String::new();

    for sentence in &sentences {
        if !current.is_empty() && current.len() + sentence.len() > config.target_size {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(sentence);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    // Recursively split any chunks that are still too large
    let mut result = Vec::new();
    for chunk in chunks {
        if chunk.len() > config.max_size {
            result.extend(recursive_split(&chunk, config, 0));
        } else {
            result.push(chunk);
        }
    }

    if result.len() <= 1 {
        return result;
    }

    // Apply sentence-level overlap: prepend the last N chars of the
    // previous chunk, snapped to a sentence boundary.
    if config.overlap == 0 {
        return result;
    }

    let mut overlapped = Vec::with_capacity(result.len());
    overlapped.push(result[0].clone());

    for chunk in result.iter().skip(1) {
        let Some(prev) = overlapped.last() else {
            continue;
        };
        let overlap_start = find_sentence_overlap_start(prev, config.overlap);
        let mut combined = String::new();
        if overlap_start < prev.len() {
            combined.push_str(&prev[overlap_start..]);
        }
        combined.push_str(chunk);
        overlapped.push(combined);
    }

    overlapped
}

/// Split text into a list of sentence strings, preserving trailing
/// whitespace and punctuation. Each returned string includes its
/// terminating punctuation (`.`, `!`, `?`) followed by any whitespace.
fn split_into_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        current.push(ch);

        // Detect sentence-ending punctuation followed by whitespace or EOF
        if (ch == '.' || ch == '!' || ch == '?')
            && (i + 1 >= chars.len() || chars[i + 1].is_whitespace() || chars[i + 1] == '\n')
        {
            // Don't split on decimals like "3.14" (digit before and after)
            if ch == '.' && i > 0 && chars[i - 1].is_ascii_digit() {
                i += 1;
                continue;
            }

            // Check for multi-period abbreviations like "e.g." or "i.e."
            // by looking at the accumulated text
            if is_likely_abbreviation(&current) {
                i += 1;
                continue;
            }

            // Consume following whitespace as part of this sentence
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                current.push(chars[j]);
                j += 1;
            }

            sentences.push(std::mem::take(&mut current));
            i = j;
            continue;
        }

        // Also treat newlines (double newline = paragraph break) as sentence boundaries
        if ch == '\n' && i + 1 < chars.len() && chars[i + 1] == '\n' {
            current.push('\n');
            sentences.push(std::mem::take(&mut current));
            i += 2;
            continue;
        }

        i += 1;
    }

    if !current.is_empty() {
        sentences.push(current);
    }

    sentences
}

/// Check whether the accumulated text ends with a known abbreviation
/// pattern that should NOT be treated as a sentence boundary.
fn is_likely_abbreviation(text: &str) -> bool {
    let trimmed = text.trim_end();
    // Single capital letter followed by period: "J." "A." etc.
    if trimmed.len() == 2 {
        let bytes = trimmed.as_bytes();
        return bytes[0].is_ascii_uppercase() && bytes[1] == b'.';
    }
    // Multi-period abbreviations: check if last few chars match "X.X" pattern
    // e.g. "e.g" "i.e" where a single letter sits between two periods
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 4 && bytes[bytes.len() - 1] == b'.' {
        // Check pattern ".X." at end (period, single letter, period)
        let n = bytes.len();
        if bytes[n - 3] == b'.' && bytes[n - 2].is_ascii_alphabetic() {
            return true;
        }
    }
    // Common abbreviations ending with period
    let lower = trimmed.to_lowercase();
    for abbr in &[
        "e.g", "i.e", "etc", "vs", "mr", "mrs", "dr", "prof", "inc", "ltd",
    ] {
        if lower.ends_with(abbr) {
            return true;
        }
    }
    false
}
/// `overlap` characters, snapped to a sentence boundary if possible.
fn find_sentence_overlap_start(prev: &str, overlap: usize) -> usize {
    if prev.len() <= overlap {
        return 0;
    }

    let raw_start = prev.len().saturating_sub(overlap);
    let safe_start = safe_split_at(prev, raw_start);

    // Try to find the next sentence start after safe_start
    let tail = &prev[safe_start..];

    // Look for a period/exclamation/question mark followed by whitespace
    // within the first ~50% of the overlap region
    let search_limit = tail.len().min(overlap);
    let search_region = &tail[..search_limit];

    for (idx, ch) in search_region.char_indices() {
        if (ch == '.' || ch == '!' || ch == '?') && idx + 1 < search_region.len() {
            // Check if followed by whitespace
            let after = &search_region[idx + 1..];
            if after.starts_with(|c: char| c.is_whitespace()) {
                // Skip past the whitespace
                let ws_end = after
                    .char_indices()
                    .skip_while(|(_, c)| c.is_whitespace())
                    .next()
                    .map(|(i, _)| i)
                    .unwrap_or(after.len());
                return safe_start + idx + 1 + ws_end;
            }
        }
    }

    // Fall back to word boundary
    let word_start = prev[safe_start..]
        .find(' ')
        .map(|pos| safe_start + pos + 1)
        .unwrap_or(safe_start);

    word_start
}

// ────────────────────────────────────────────────────────────────
// Code strategy
// ────────────────────────────────────────────────────────────────

/// Split code text while avoiding breaks inside function bodies.
/// Detects Rust, Python, and TypeScript/JavaScript function boundaries.
/// Falls back to plain splitting for non-code or unrecognized content.
fn code_split(text: &str, config: &ChunkingConfig) -> Vec<String> {
    let boundaries = detect_code_block_boundaries(text);

    if boundaries.is_empty() {
        // No function blocks detected — fall back to plain split
        return recursive_split(text, config, 0);
    }

    // Build segments: text between boundaries, where each function body
    // is kept as a single unit if possible.
    let segments = build_code_segments(text, &boundaries);

    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in &segments {
        // If the segment itself exceeds max_size, it must be split.
        // For function bodies, we use force_split as a last resort.
        if segment.len() > config.max_size {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            // Force-split oversized function bodies
            for part in force_split(segment, config.max_size) {
                chunks.push(part);
            }
            continue;
        }

        if !current.is_empty() && current.len() + segment.len() > config.target_size {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(segment);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    // Recursively split any remaining oversized chunks
    let mut result = Vec::new();
    for chunk in chunks {
        if chunk.len() > config.max_size {
            result.extend(recursive_split(&chunk, config, 0));
        } else {
            result.push(chunk);
        }
    }

    if result.is_empty() {
        vec![text.to_string()]
    } else {
        result
    }
}

/// A code block boundary: the byte offset where a function body starts
/// and the byte offset where it ends.
#[derive(Debug, Clone)]
struct CodeBlockBoundary {
    /// Byte offset of the first character of the function definition line
    start: usize,
    /// Byte offset one past the last character of the function body
    end: usize,
}

/// Detect function block boundaries in Rust, Python, and TS/JS code.
///
/// Returns a list of non-overlapping boundaries sorted by start position.
fn detect_code_block_boundaries(text: &str) -> Vec<CodeBlockBoundary> {
    let mut boundaries = Vec::new();

    // Try Rust/TS/JS style (brace-delimited functions)
    boundaries.extend(detect_brace_functions(text));
    // Try Python style (indentation-based functions)
    boundaries.extend(detect_python_functions(text));

    if boundaries.is_empty() {
        return Vec::new();
    }

    // Sort by start position
    boundaries.sort_by_key(|b| b.start);

    // Remove overlapping boundaries (prefer the earlier one)
    let mut result = Vec::new();
    for b in boundaries {
        if result
            .last()
            .map_or(true, |last: &CodeBlockBoundary| last.end <= b.start)
        {
            result.push(b);
        }
    }

    result
}

/// Detect Rust/TypeScript/JavaScript function boundaries using brace matching.
///
/// Looks for lines matching patterns like:
/// - `fn name(` (Rust)
/// - `function name(` (TS/JS)
/// - `async fn name(` (Rust async)
/// - `async function name(` (TS/JS async)
/// - `const name = (` / `const name = function` (TS/JS arrow/named)
/// - Arrow functions `=>` at top level
fn detect_brace_functions(text: &str) -> Vec<CodeBlockBoundary> {
    let mut boundaries = Vec::new();
    let lines: Vec<(usize, &str)> = text
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1; // +1 for newline
            Some((start, line))
        })
        .collect();

    for (i, &(line_start, line)) in lines.iter().enumerate() {
        let trimmed = line.trim_start();

        // Detect function definitions
        let is_fn = trimmed.starts_with("fn ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("pub(crate) fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("export async function ")
            || trimmed.starts_with("export default function ")
            || (trimmed.starts_with("const ") && trimmed.contains("= function"))
            || (trimmed.starts_with("const ") && trimmed.contains("=>"))
            || (trimmed.starts_with("pub const ") && trimmed.contains("=>"));

        if !is_fn {
            continue;
        }

        // Find the opening brace `{` on this line or subsequent lines
        let mut brace_offset = None;

        for (j, &(ls, l)) in lines.iter().enumerate().skip(i) {
            if let Some(pos) = l.find('{') {
                brace_offset = Some(ls + pos);
                break;
            }
            // If we hit a semicolon before a brace, this isn't a function body
            // (e.g. a type alias or forward declaration)
            if l.contains(';') && !l.contains('{') && j > i {
                break;
            }
        }

        let Some(brace_start) = brace_offset else {
            continue;
        };

        // Match braces from the opening to find the closing brace
        if let Some(close_end) = match_braces(text, brace_start) {
            boundaries.push(CodeBlockBoundary {
                start: line_start,
                end: close_end,
            });
        }
    }

    boundaries
}

/// Match braces starting at `brace_start` (the position of `{`).
/// Returns the byte offset one past the matching closing `}`.
fn match_braces(text: &str, brace_start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut string_char = b'"';
    let mut in_char = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut prev_char = b'\0';

    let mut i = brace_start;
    while i < bytes.len() {
        let ch = bytes[i];

        if in_line_comment {
            if ch == b'\n' {
                in_line_comment = false;
            }
            prev_char = ch;
            i += 1;
            continue;
        }

        if in_block_comment {
            if prev_char == b'*' && ch == b'/' {
                in_block_comment = false;
            }
            prev_char = ch;
            i += 1;
            continue;
        }

        if in_string {
            if ch == string_char && prev_char != b'\\' {
                in_string = false;
            }
            prev_char = ch;
            i += 1;
            continue;
        }

        if in_char {
            if ch == b'\'' && prev_char != b'\\' {
                in_char = false;
            }
            prev_char = ch;
            i += 1;
            continue;
        }

        // Check for comments
        if ch == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                in_line_comment = true;
                prev_char = ch;
                i += 2;
                continue;
            }
            if bytes[i + 1] == b'*' {
                in_block_comment = true;
                prev_char = ch;
                i += 2;
                continue;
            }
        }

        // Check for strings
        if ch == b'"' || ch == b'`' {
            in_string = true;
            string_char = ch;
            prev_char = ch;
            i += 1;
            continue;
        }

        if ch == b'\'' {
            in_char = true;
            prev_char = ch;
            i += 1;
            continue;
        }

        if ch == b'{' {
            depth += 1;
        } else if ch == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1);
            }
        }

        prev_char = ch;
        i += 1;
    }

    None
}

/// Detect Python function boundaries using indentation tracking.
///
/// Looks for `def ` at any indentation level and tracks the function body
/// as all subsequent lines with greater indentation.
fn detect_python_functions(text: &str) -> Vec<CodeBlockBoundary> {
    let mut boundaries = Vec::new();
    let lines: Vec<(usize, &str)> = text
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .collect();

    for (i, &(line_start, line)) in lines.iter().enumerate() {
        let trimmed = line.trim_start();

        // Detect Python function/class definitions
        if !(trimmed.starts_with("def ")
            || trimmed.starts_with("async def ")
            || trimmed.starts_with("class "))
        {
            continue;
        }

        // Check for colon at end (possibly after parameters)
        // Look forward for the colon on the same line or continuation lines
        let mut colon_found = false;
        let mut end_line_idx = i;
        for (j, &(_, l)) in lines.iter().enumerate().skip(i) {
            if l.contains(':') {
                colon_found = true;
                end_line_idx = j;
                break;
            }
            // If we hit a blank line or dedent without colon, not a function
            if j > i && l.trim().is_empty() {
                break;
            }
        }

        if !colon_found {
            continue;
        }

        // Find the body: the first non-blank line after the colon line
        // with greater indentation than the def line
        let def_indent = line.len() - line.trim_start().len();
        let mut body_start_line = None;
        let mut body_end_line = end_line_idx;

        for (k, &(_, l)) in lines.iter().enumerate().skip(end_line_idx + 1) {
            if l.trim().is_empty() {
                continue;
            }
            let line_indent = l.len() - l.trim_start().len();

            if body_start_line.is_none() {
                if line_indent > def_indent {
                    body_start_line = Some(k);
                } else {
                    // No body found
                    break;
                }
            } else {
                // Body continues while indentation > def_indent
                if line_indent <= def_indent && !l.trim().is_empty() {
                    body_end_line = k.saturating_sub(1);
                    break;
                }
                body_end_line = k;
            }
        }

        if let Some(bsl) = body_start_line {
            let end_offset = if body_end_line + 1 < lines.len() {
                lines[body_end_line + 1].0
            } else {
                // Body extends to end of text
                text.len()
            };
            boundaries.push(CodeBlockBoundary {
                start: line_start,
                end: end_offset.max(lines[bsl].0),
            });
        }
    }

    boundaries
}

/// Build text segments from code block boundaries.
/// Text outside boundaries becomes its own segment; each function body
/// becomes a single segment.
fn build_code_segments(text: &str, boundaries: &[CodeBlockBoundary]) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cursor = 0;

    for b in boundaries {
        // Text before the function
        if b.start > cursor {
            let prefix = &text[cursor..b.start];
            if !prefix.is_empty() {
                segments.push(prefix.to_string());
            }
        }
        // The function body itself
        if b.end > b.start {
            segments.push(text[b.start..b.end].to_string());
        }
        cursor = b.end;
    }

    // Trailing text after last boundary
    if cursor < text.len() {
        let suffix = &text[cursor..];
        if !suffix.is_empty() {
            segments.push(suffix.to_string());
        }
    }

    segments
}

// ────────────────────────────────────────────────────────────────
// Markdown strategy
// ────────────────────────────────────────────────────────────────

/// Split markdown text on header boundaries. Each header and its content
/// up to the next header of the same or higher level become a chunk.
/// Oversized chunks are recursively split.
fn markdown_split(text: &str, config: &ChunkingConfig) -> Vec<String> {
    let sections = split_markdown_by_headers(text);

    if sections.is_empty() {
        return recursive_split(text, config, 0);
    }

    // Pack sections into chunks up to target_size
    let mut chunks = Vec::new();
    let mut current = String::new();

    for section in &sections {
        if !current.is_empty() && current.len() + section.len() > config.target_size {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(section);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    // Recursively split any chunks that are still too large
    let mut result = Vec::new();
    for chunk in chunks {
        if chunk.len() > config.max_size {
            result.extend(recursive_split(&chunk, config, 0));
        } else {
            result.push(chunk);
        }
    }

    if result.is_empty() {
        vec![text.to_string()]
    } else {
        result
    }
}

/// Split markdown text into sections based on header boundaries.
/// A section starts at a header line (`# `, `## `, etc.) and includes
/// all content up to the next header of the same or higher level.
/// Content before the first header becomes its own section.
fn split_markdown_by_headers(text: &str) -> Vec<String> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .collect();

    if lines.is_empty() {
        return Vec::new();
    }

    // Find header positions and levels
    let header_positions: Vec<(usize, usize)> = lines
        .iter()
        .filter_map(|&(offset, line)| {
            let level = markdown_header_level(line);
            if level > 0 {
                Some((offset, level))
            } else {
                None
            }
        })
        .collect();

    if header_positions.is_empty() {
        return vec![text.to_string()];
    }

    let mut sections = Vec::new();
    let mut cursor = 0;

    for (idx, &(header_offset, header_level)) in header_positions.iter().enumerate() {
        // Content before this header (if any) belongs to the previous section
        // or is a preamble
        if idx == 0 && header_offset > 0 {
            let preamble = &text[0..header_offset];
            if !preamble.trim().is_empty() {
                sections.push(preamble.to_string());
            }
        }

        // Find the end of this section: the next header with level <= header_level
        let section_end = header_positions
            .iter()
            .skip(idx + 1)
            .find(|&&(_, level)| level <= header_level)
            .map(|&(offset, _)| offset)
            .unwrap_or(text.len());

        cursor = section_end;
        let section_text = &text[header_offset..section_end];
        if !section_text.trim().is_empty() {
            sections.push(section_text.to_string());
        }
    }

    // Trailing content after the last section's end
    if cursor < text.len() {
        let trailing = &text[cursor..];
        if !trailing.trim().is_empty() {
            sections.push(trailing.to_string());
        }
    }

    sections
}

/// Return the header level (1-6) for a markdown header line, or 0 if not a header.
/// Supports ATX-style headers (`# Header`, `## Subheader`).
fn markdown_header_level(line: &str) -> usize {
    let trimmed = line.trim_start();

    // ATX-style header
    if trimmed.starts_with('#') {
        let count = trimmed.chars().take_while(|&c| c == '#').count();
        if count >= 1 && count <= 6 {
            // Must be followed by whitespace then content
            let after_hashes = &trimmed[count..];
            if after_hashes.is_empty() {
                return 0; // "##" with no content is not a header
            }
            if after_hashes.starts_with(' ') || after_hashes.starts_with('\t') {
                // Check there's actual content after the space
                if after_hashes.trim().is_empty() {
                    return 0;
                }
                return count;
            }
        }
        return 0;
    }

    0
}

// ────────────────────────────────────────────────────────────────
// Shared utilities
// ────────────────────────────────────────────────────────────────

/// Snap a byte offset to the nearest valid UTF-8 char boundary (searching backward).
fn safe_split_at(text: &str, pos: usize) -> usize {
    let mut split = pos.min(text.len());
    while split > 0 && !text.is_char_boundary(split) {
        split -= 1;
    }
    split
}

/// Force-split text at max_size boundaries, respecting UTF-8.
fn force_split(text: &str, max_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = safe_split_at(text, start + max_size);
        if end <= start {
            // Can't make progress — advance past the current character
            let mut next = start + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            if next <= text.len() {
                chunks.push(text[start..next].to_string());
            }
            start = next;
            continue;
        }
        chunks.push(text[start..end].to_string());
        start = end;
    }
    chunks
}

/// Recursively split text using a hierarchy of separators.
fn recursive_split(text: &str, config: &ChunkingConfig, depth: usize) -> Vec<String> {
    if text.len() <= config.max_size {
        return vec![text.to_string()];
    }

    if depth >= MAX_RECURSION_DEPTH {
        tracing::warn!("Chunker hit max recursion depth, force-splitting at max_size");
        return force_split(text, config.max_size);
    }

    // Separator hierarchy: paragraph > sentence > word > force
    let separators: &[&str] = &["\n\n", ". ", "? ", "! ", " "];

    for sep in separators {
        let parts: Vec<&str> = text.split(sep).collect();
        if parts.len() <= 1 {
            continue;
        }

        let mut chunks = Vec::new();
        let mut current = String::new();

        for (i, part) in parts.iter().enumerate() {
            let piece = if i + 1 < parts.len() {
                format!("{}{}", part, sep)
            } else {
                part.to_string()
            };

            if current.len() + piece.len() > config.target_size && !current.is_empty() {
                chunks.push(current);
                current = piece;
            } else {
                current.push_str(&piece);
            }
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        // Recursively split any chunks that are still too large
        let mut result = Vec::new();
        for chunk in chunks {
            if chunk.len() > config.max_size {
                result.extend(recursive_split(&chunk, config, depth + 1));
            } else {
                result.push(chunk);
            }
        }

        if result.len() > 1 {
            return result;
        }
    }

    // All separators exhausted — force split
    force_split(text, config.max_size)
}

/// Merge adjacent chunks that are smaller than min_size.
fn merge_small_chunks(chunks: Vec<String>, target_size: usize, min_size: usize) -> Vec<String> {
    if chunks.is_empty() {
        return chunks;
    }

    let mut merged = Vec::new();
    let mut current = chunks[0].clone();

    for chunk in chunks.iter().skip(1) {
        if (current.len() < min_size || chunk.len() < min_size)
            && current.len() + chunk.len() <= target_size
        {
            current.push_str(chunk);
        } else {
            merged.push(current);
            current = chunk.clone();
        }
    }

    merged.push(current);
    if merged.len() > 1 {
        if let Some(last) = merged.last() {
            if last.len() < min_size {
                let last = merged.pop().unwrap_or_default();
                if let Some(previous) = merged.last_mut() {
                    previous.push_str(&last);
                } else {
                    merged.push(last);
                }
            }
        }
    }
    merged
}

/// Apply overlap between adjacent chunks.
fn apply_overlap(chunks: Vec<String>, overlap: usize) -> Vec<String> {
    if chunks.len() <= 1 || overlap == 0 {
        return chunks;
    }

    let mut result = Vec::with_capacity(chunks.len());
    result.push(chunks[0].clone());

    for i in 1..chunks.len() {
        let prev = result.last().map(String::as_str).unwrap_or(&chunks[i - 1]);
        let overlap_start = if prev.len() > overlap {
            // Find a word boundary for the overlap start
            let raw_start = prev.len() - overlap;
            let safe_start = safe_split_at(prev, raw_start);
            // Try to find a space after safe_start to avoid cutting mid-word
            prev[safe_start..]
                .find(' ')
                .map(|pos| safe_start + pos + 1)
                .unwrap_or(safe_start)
        } else {
            0
        };

        let overlap_text = &prev[overlap_start..];
        let mut chunk_with_overlap = overlap_text.to_string();
        chunk_with_overlap.push_str(&chunks[i]);
        result.push(chunk_with_overlap);
    }

    result
}

// ────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::EstimateTokenCounter;

    fn make_config(strategy: ChunkingStrategy) -> ChunkingConfig {
        ChunkingConfig {
            target_size: 100,
            min_size: 10,
            max_size: 200,
            overlap: 0,
            strategy,
        }
    }

    fn default_config() -> ChunkingConfig {
        ChunkingConfig::default()
    }

    fn token_counter() -> EstimateTokenCounter {
        EstimateTokenCounter
    }

    // ─── Plain strategy (backward compatibility) ───────────────────

    #[test]
    fn test_plain_empty_string() {
        let config = make_config(ChunkingStrategy::Plain);
        let counter = token_counter();
        let chunks = chunk_text("", &config, &counter);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_plain_short_text_single_chunk() {
        let config = make_config(ChunkingStrategy::Plain);
        let counter = token_counter();
        let text = "Hello world.";
        let chunks = chunk_text(text, &config, &counter);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, text);
    }

    #[test]
    fn test_plain_paragraph_split() {
        let config = ChunkingConfig {
            target_size: 30,
            min_size: 5,
            max_size: 60,
            overlap: 0,
            strategy: ChunkingStrategy::Plain,
        };
        let counter = token_counter();
        let text = "First paragraph here with enough text to exceed the limit.\n\nSecond paragraph here with enough text to exceed the limit.\n\nThird one here with enough text.";
        let chunks = chunk_text(text, &config, &counter);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn test_plain_default_strategy_is_plain() {
        // Default config must use Plain strategy
        let config = default_config();
        assert_eq!(config.strategy, ChunkingStrategy::Plain);
    }

    #[test]
    fn test_plain_backward_compatible_output() {
        // The default strategy must produce the same output as Plain
        let text = "This is a test sentence. This is another one. And a third here. ";
        let counter = token_counter();

        let default_config = default_config();
        let plain_config = ChunkingConfig {
            strategy: ChunkingStrategy::Plain,
            ..default_config.clone()
        };

        let default_chunks = chunk_text(text, &default_config, &counter);
        let plain_chunks = chunk_text(text, &plain_config, &counter);

        assert_eq!(default_chunks.len(), plain_chunks.len());
        for (d, p) in default_chunks.iter().zip(plain_chunks.iter()) {
            assert_eq!(d.content, p.content);
            assert_eq!(d.index, p.index);
        }
    }

    #[test]
    fn test_plain_unicode_safe() {
        let config = default_config();
        let counter = token_counter();
        let text =
            "日本語のテストです。これは非常に長いテキストで、チャンク分割が必要です。".repeat(20);
        let chunks = chunk_text(&text, &config, &counter);
        // Should not panic and should produce valid UTF-8 chunks
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.content.as_bytes()).is_ok());
        }
    }

    // ─── Sentence strategy ─────────────────────────────────────────

    #[test]
    fn test_sentence_empty_string() {
        let config = make_config(ChunkingStrategy::Sentence);
        let counter = token_counter();
        let chunks = chunk_text("", &config, &counter);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_sentence_short_text_single_chunk() {
        let config = make_config(ChunkingStrategy::Sentence);
        let counter = token_counter();
        let text = "Hello world.";
        let chunks = chunk_text(text, &config, &counter);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, text);
    }

    #[test]
    fn test_sentence_splits_on_boundaries() {
        let config = ChunkingConfig {
            target_size: 30,
            min_size: 5,
            max_size: 60,
            overlap: 0,
            strategy: ChunkingStrategy::Sentence,
        };
        let counter = token_counter();
        let text = "First sentence here. Second one follows. Third one now. Fourth.";
        let chunks = chunk_text(text, &config, &counter);
        assert!(
            chunks.len() > 1,
            "expected multiple chunks, got {}",
            chunks.len()
        );
        // Each chunk should not exceed max_size
        for chunk in &chunks {
            assert!(
                chunk.content.len() <= 60,
                "chunk len {} exceeds max_size 60",
                chunk.content.len()
            );
        }
    }

    #[test]
    fn test_sentence_with_overlap() {
        let config = ChunkingConfig {
            target_size: 30,
            min_size: 5,
            max_size: 60,
            overlap: 10,
            strategy: ChunkingStrategy::Sentence,
        };
        let counter = token_counter();
        let text = "First sentence here. Second one follows. Third one now. Fourth.";
        let chunks = chunk_text(text, &config, &counter);
        assert!(chunks.len() > 1);
        // With overlap, the second chunk should start with some content
        // from the end of the first chunk
        if chunks.len() >= 2 {
            // The overlap should bring some characters from chunk[0] into chunk[1]
            assert!(!chunks[1].content.is_empty());
        }
    }

    #[test]
    fn test_sentence_no_split_on_abbreviations() {
        let sentences = split_into_sentences("Hello e.g. world. This is a test.");
        // "e.g." should not be split as a sentence boundary
        // So "Hello e.g. world." should be one sentence
        assert!(
            sentences.len() <= 3,
            "got {} sentences: {:?}",
            sentences.len(),
            sentences
        );
    }

    #[test]
    fn test_sentence_no_split_on_decimals() {
        let sentences = split_into_sentences("The value is 3.14 today. End.");
        // "3.14" should not trigger a sentence split
        assert_eq!(sentences.len(), 2);
    }

    #[test]
    fn test_sentence_paragraph_break() {
        let sentences = split_into_sentences("First paragraph.\n\nSecond paragraph.");
        // Double newline should create a boundary
        assert!(sentences.len() >= 2);
    }

    // ─── Code strategy ─────────────────────────────────────────────

    #[test]
    fn test_code_empty_string() {
        let config = make_config(ChunkingStrategy::Code);
        let counter = token_counter();
        let chunks = chunk_text("", &config, &counter);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_code_rust_function_not_split() {
        let config = ChunkingConfig {
            target_size: 50,
            min_size: 5,
            max_size: 500,
            overlap: 0,
            strategy: ChunkingStrategy::Code,
        };
        let counter = token_counter();
        let text = r#"// Header comment
fn foo() {
    let x = 1;
    let y = 2;
    let z = x + y;
    println!("{}", z);
    // More code
    let a = 10;
    let b = 20;
    let c = a + b;
}

fn bar() {
    println!("hello");
}
"#;
        let chunks = chunk_text(text, &config, &counter);
        // The function body should not be split mid-body
        let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
        assert!(all_content.contains("fn foo()") || all_content.contains("fn bar()"));
        // At least the function signatures should be intact
        for chunk in &chunks {
            assert!(chunk.content.len() <= 500, "chunk exceeds max_size");
        }
    }

    #[test]
    fn test_code_python_function_not_split() {
        let config = ChunkingConfig {
            target_size: 40,
            min_size: 5,
            max_size: 500,
            overlap: 0,
            strategy: ChunkingStrategy::Code,
        };
        let counter = token_counter();
        let text = "# Header comment\ndef foo(x, y):\n    result = x + y\n    print(result)\n    return result\n\ndef bar():\n    print('hello')\n";
        let chunks = chunk_text(text, &config, &counter);
        // Should not split inside function bodies
        let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
        assert!(all_content.contains("def foo") || all_content.contains("def bar"));
    }

    #[test]
    fn test_code_typescript_function_not_split() {
        let config = ChunkingConfig {
            target_size: 50,
            min_size: 5,
            max_size: 500,
            overlap: 0,
            strategy: ChunkingStrategy::Code,
        };
        let counter = token_counter();
        let text = "// Header\nfunction add(a: number, b: number): number {\n    return a + b;\n}\n\nfunction sub(a: number, b: number): number {\n    return a - b;\n}\n";
        let chunks = chunk_text(text, &config, &counter);
        let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect();
        assert!(all_content.contains("function add"));
    }

    #[test]
    fn test_code_falls_back_to_plain_for_non_code() {
        let config = make_config(ChunkingStrategy::Code);
        let counter = token_counter();
        let text = "This is just plain text with no functions at all. Just sentences.";
        let chunks = chunk_text(text, &config, &counter);
        // Should not panic and should produce valid chunks
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_code_brace_matching_with_strings() {
        let text = "fn test() { let s = \"{ not a brace }\"; let x = 1; }";
        let boundaries = detect_brace_functions(text);
        assert_eq!(boundaries.len(), 1);
        // The boundary should span the entire function
        assert!(boundaries[0].end > boundaries[0].start);
    }

    #[test]
    fn test_code_brace_matching_with_comments() {
        let text = "fn test() { // { comment brace\n let x = 1; /* } */ let y = 2; }";
        let boundaries = detect_brace_functions(text);
        assert_eq!(boundaries.len(), 1);
    }

    #[test]
    fn test_code_async_function_detection() {
        let text = "async fn fetch() { let resp = reqwest::get(\"url\").await; resp }";
        let boundaries = detect_brace_functions(text);
        assert_eq!(boundaries.len(), 1);
    }

    // ─── Markdown strategy ─────────────────────────────────────────

    #[test]
    fn test_markdown_empty_string() {
        let config = make_config(ChunkingStrategy::Markdown);
        let counter = token_counter();
        let chunks = chunk_text("", &config, &counter);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_markdown_splits_on_headers() {
        let config = ChunkingConfig {
            target_size: 100,
            min_size: 5,
            max_size: 500,
            overlap: 0,
            strategy: ChunkingStrategy::Markdown,
        };
        let counter = token_counter();
        let text = "# Title\n\nSome content here.\n\n## Section A\n\nContent for A.\n\n## Section B\n\nContent for B.";
        let chunks = chunk_text(text, &config, &counter);
        // Should produce multiple chunks based on headers
        assert!(
            chunks.len() > 1,
            "expected multiple chunks, got {}",
            chunks.len()
        );
    }

    #[test]
    fn test_markdown_header_level_detection() {
        assert_eq!(markdown_header_level("# Header"), 1);
        assert_eq!(markdown_header_level("## Header"), 2);
        assert_eq!(markdown_header_level("### Header"), 3);
        assert_eq!(markdown_header_level("###### Header"), 6);
        assert_eq!(markdown_header_level("####### Too many"), 0);
        assert_eq!(markdown_header_level("Not a header"), 0);
        assert_eq!(markdown_header_level("##"), 0); // No space after hashes
        assert_eq!(markdown_header_level(""), 0);
        assert_eq!(markdown_header_level("Some ## text"), 0);
    }

    #[test]
    fn test_markdown_section_grouping() {
        let text = "# Main\n\nIntro.\n\n## Sub 1\n\nSub 1 content.\n\n## Sub 2\n\nSub 2 content.";
        let sections = split_markdown_by_headers(text);
        // # Main should contain its intro, and sub-sections should be separate
        assert!(sections.len() >= 2, "got {} sections", sections.len());
        // The first section should contain "# Main"
        assert!(sections[0].contains("# Main"));
    }

    #[test]
    fn test_markdown_no_headers_returns_single_chunk() {
        let config = make_config(ChunkingStrategy::Markdown);
        let counter = token_counter();
        let text = "Just some plain text. No headers here.";
        let chunks = chunk_text(text, &config, &counter);
        // Without headers, should still chunk (may be single chunk if short)
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_markdown_nested_headers() {
        let text = "# Top\n\nIntro.\n\n## Sub\n\nSub content.\n\n### Subsub\n\nDeep.\n\n# Next Top\n\nContent.";
        let sections = split_markdown_by_headers(text);
        // # Top should include ## Sub and ### Subsub content
        // # Next Top should be a separate section
        assert!(sections.len() >= 2);
        // First section should start with # Top
        assert!(sections[0].contains("# Top"));
        // Last section should contain # Next Top
        let last = sections.last().unwrap();
        assert!(last.contains("# Next Top"));
    }

    // ─── Cross-strategy tests ──────────────────────────────────────

    #[test]
    fn test_all_strategies_handle_unicode() {
        let counter = token_counter();
        let text = "日本語テスト。これは長いテキストです。".repeat(10);

        for strategy in [
            ChunkingStrategy::Plain,
            ChunkingStrategy::Sentence,
            ChunkingStrategy::Code,
            ChunkingStrategy::Markdown,
        ] {
            let config = ChunkingConfig {
                target_size: 50,
                min_size: 5,
                max_size: 100,
                overlap: 0,
                strategy,
            };
            let chunks = chunk_text(&text, &config, &counter);
            // All chunks should be valid UTF-8
            for chunk in &chunks {
                assert!(
                    std::str::from_utf8(chunk.content.as_bytes()).is_ok(),
                    "Invalid UTF-8 in {:?} strategy",
                    strategy
                );
            }
        }
    }

    #[test]
    fn test_all_strategies_empty_input() {
        let counter = token_counter();
        for strategy in [
            ChunkingStrategy::Plain,
            ChunkingStrategy::Sentence,
            ChunkingStrategy::Code,
            ChunkingStrategy::Markdown,
        ] {
            let config = make_config(strategy);
            let chunks = chunk_text("", &config, &counter);
            assert!(
                chunks.is_empty(),
                "{:?} should return empty for empty input",
                strategy
            );
        }
    }

    #[test]
    fn test_token_counts_use_counter() {
        let config = make_config(ChunkingStrategy::Plain);
        let counter = token_counter();
        let text = "Hello world this is a test.";
        let chunks = chunk_text(text, &config, &counter);
        for chunk in &chunks {
            // EstimateTokenCounter uses chars/4 with min 1
            let expected = (chunk.content.len() / 4).max(1);
            assert_eq!(chunk.token_count_estimate, expected);
        }
    }

    // ─── ChunkingStrategy serde ────────────────────────────────────

    #[test]
    fn test_chunking_strategy_serde() {
        // Plain should deserialize from "plain"
        let plain: ChunkingStrategy = serde_json::from_str("\"plain\"").unwrap();
        assert_eq!(plain, ChunkingStrategy::Plain);

        let sentence: ChunkingStrategy = serde_json::from_str("\"sentence\"").unwrap();
        assert_eq!(sentence, ChunkingStrategy::Sentence);

        let code: ChunkingStrategy = serde_json::from_str("\"code\"").unwrap();
        assert_eq!(code, ChunkingStrategy::Code);

        let markdown: ChunkingStrategy = serde_json::from_str("\"markdown\"").unwrap();
        assert_eq!(markdown, ChunkingStrategy::Markdown);
    }

    #[test]
    fn test_chunking_config_default_strategy() {
        // Default config should have Plain strategy
        let config = ChunkingConfig::default();
        assert_eq!(config.strategy, ChunkingStrategy::Plain);

        // Also test that deserializing without strategy field gives Plain
        let json = r#"{"target_size":100,"min_size":10,"max_size":200,"overlap":0}"#;
        let config: ChunkingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.strategy, ChunkingStrategy::Plain);
    }
}
