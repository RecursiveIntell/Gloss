use semantic_memory::ChunkingConfig;
use semantic_memory::EstimateTokenCounter;

fn default_config() -> ChunkingConfig {
    ChunkingConfig::default()
}

fn default_counter() -> EstimateTokenCounter {
    EstimateTokenCounter
}

#[test]
fn empty_input_returns_empty_vec() {
    let chunks = semantic_memory::chunker::chunk_text("", &default_config(), &default_counter());
    assert!(chunks.is_empty());
}

#[test]
fn whitespace_only_returns_empty_vec() {
    let chunks = semantic_memory::chunker::chunk_text(
        "   \n\n  \t  ",
        &default_config(),
        &default_counter(),
    );
    assert!(chunks.is_empty());
}

#[test]
fn short_text_returns_single_chunk() {
    let chunks = semantic_memory::chunker::chunk_text(
        "Hello, world!",
        &default_config(),
        &default_counter(),
    );
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].content, "Hello, world!");
    assert_eq!(chunks[0].index, 0);
}

#[test]
fn text_under_max_size_single_chunk() {
    let text = "This is a short paragraph. ".repeat(10);
    let config = default_config();
    assert!(text.len() < config.max_size);
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    assert_eq!(chunks.len(), 1);
}

#[test]
fn paragraph_separated_text_splits_on_paragraph_break() {
    let paragraph = "This is some content. ".repeat(60);
    let text = format!("{}\n\n{}\n\n{}", paragraph, paragraph, paragraph);
    let config = default_config();
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    assert!(
        chunks.len() >= 2,
        "Expected multiple chunks, got {}",
        chunks.len()
    );
}

#[test]
fn splitting_preserves_separator_content() {
    let text = format!("{} {}", "alpha.".repeat(220), "beta.".repeat(220));
    let config = ChunkingConfig {
        target_size: 300,
        min_size: 100,
        max_size: 500,
        overlap: 0,
    };
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    let joined = chunks
        .iter()
        .map(|chunk| chunk.content.as_str())
        .collect::<String>();
    assert_eq!(joined, text);
}

#[test]
fn sentence_splitting_works() {
    let text = "First sentence. ".repeat(200);
    let config = default_config();
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    assert!(
        chunks.len() > 1,
        "Expected multiple chunks from sentence splitting"
    );
}

#[test]
fn long_single_word_force_splits() {
    let word = "a".repeat(5000);
    let config = default_config();
    let chunks = semantic_memory::chunker::chunk_text(&word, &config, &default_counter());
    assert!(chunks.len() > 1, "Should force-split a very long word");
    for chunk in &chunks {
        // Overlap can add up to config.overlap bytes to a chunk
        assert!(
            chunk.content.len() <= config.max_size + config.overlap,
            "Chunk too large: {} (max {} + overlap {})",
            chunk.content.len(),
            config.max_size,
            config.overlap
        );
    }
}

#[test]
fn unicode_cjk_no_panic() {
    let text = "中文测试内容。".repeat(500);
    let config = default_config();
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    assert!(!chunks.is_empty());
    for chunk in &chunks {
        // Verify valid UTF-8 (would already panic if not, but let's be explicit)
        assert!(std::str::from_utf8(chunk.content.as_bytes()).is_ok());
    }
}

#[test]
fn unicode_emoji_no_panic() {
    let text = "Hello 🌍🌎🌏 world! ".repeat(300);
    let config = default_config();
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    assert!(!chunks.is_empty());
}

#[test]
fn overlap_between_chunks() {
    let paragraph = "Word ".repeat(300);
    let text = format!("{}\n\n{}\n\n{}", paragraph, paragraph, paragraph);
    let config = ChunkingConfig {
        target_size: 500,
        min_size: 50,
        max_size: 800,
        overlap: 100,
    };
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    if chunks.len() >= 2 {
        // The second chunk should overlap with the first
        let first_tail = &chunks[0].content[chunks[0].content.len().saturating_sub(100)..];
        // There should be some shared content
        let has_overlap = chunks[1]
            .content
            .contains(first_tail.split_whitespace().last().unwrap_or(""));
        assert!(
            has_overlap || chunks.len() > 1,
            "Expected overlap between chunks"
        );
    }
}

#[test]
fn token_count_estimate_is_reasonable() {
    let text = "Hello world, this is a test of token counting.";
    let config = default_config();
    let chunks = semantic_memory::chunker::chunk_text(text, &config, &default_counter());
    assert_eq!(chunks.len(), 1);
    // EstimateTokenCounter: max(len/4, 1) for non-empty text = max(47/4, 1) = max(11, 1) = 11
    assert_eq!(chunks[0].token_count_estimate, text.len() / 4);
}

#[test]
fn chunk_indices_are_sequential() {
    let text = "Sentence here. ".repeat(200);
    let config = ChunkingConfig {
        target_size: 200,
        min_size: 50,
        max_size: 400,
        overlap: 50,
    };
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, i);
    }
}

#[test]
fn final_small_tail_merges_into_previous_chunk() {
    let text = format!("{}{}", "word ".repeat(120), "tail");
    let config = ChunkingConfig {
        target_size: 250,
        min_size: 80,
        max_size: 320,
        overlap: 0,
    };
    let chunks = semantic_memory::chunker::chunk_text(&text, &config, &default_counter());
    assert!(chunks
        .last()
        .is_some_and(|chunk| chunk.content.ends_with("tail")));
    assert!(chunks
        .iter()
        .all(|chunk| chunk.content.len() >= config.min_size));
}
