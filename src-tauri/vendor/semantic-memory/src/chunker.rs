//! Text chunking via recursive splitting with overlap.

use crate::config::ChunkingConfig;
use crate::tokenizer::TokenCounter;
use crate::types::TextChunk;

/// Maximum recursion depth to prevent infinite loops.
const MAX_RECURSION_DEPTH: usize = 10;

/// Chunk text using the given configuration and token counter.
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

    if text.len() <= config.max_size && text.len() >= config.min_size {
        return vec![TextChunk {
            index: 0,
            content: text.to_string(),
            token_count_estimate: token_counter.count_tokens(text),
        }];
    }

    // Recursive split
    let raw_chunks = recursive_split(text, config, 0);

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
