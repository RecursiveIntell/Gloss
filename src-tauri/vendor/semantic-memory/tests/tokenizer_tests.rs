use semantic_memory::{
    ChunkingConfig, EstimateTokenCounter, MemoryConfig, MemoryStore, MockEmbedder, Role,
    TokenCounter,
};
use std::sync::Arc;
use tempfile::TempDir;

// ─── EstimateTokenCounter basic behavior ────────────────────

#[test]
fn test_estimate_counter_empty() {
    let counter = EstimateTokenCounter;
    assert_eq!(counter.count_tokens(""), 0);
}

#[test]
fn test_estimate_counter_short() {
    let counter = EstimateTokenCounter;
    // len=2, /4=0, max(1)=1
    assert_eq!(counter.count_tokens("hi"), 1);
}

#[test]
fn test_estimate_counter_known_length() {
    let counter = EstimateTokenCounter;
    // "hello world test" = 16 chars, /4=4
    assert_eq!(counter.count_tokens("hello world test"), 4);
}

#[test]
fn test_estimate_counter_single_char() {
    let counter = EstimateTokenCounter;
    // len=1, /4=0, max(1)=1
    assert_eq!(counter.count_tokens("a"), 1);
}

// ─── Custom TokenCounter ────────────────────────────────────

struct WordCounter;
impl TokenCounter for WordCounter {
    fn count_tokens(&self, text: &str) -> usize {
        text.split_whitespace().count()
    }
}

#[tokio::test]
async fn test_custom_token_counter() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        token_counter: Some(Arc::new(WordCounter)),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let session = store.create_session("test").await.unwrap();
    // Add a message with token_count = None — should use WordCounter
    store
        .add_message(&session, Role::User, "one two three four five", None, None)
        .await
        .unwrap();

    let messages = store.get_recent_messages(&session, 10).await.unwrap();
    // WordCounter counts whitespace-separated words: 5
    assert_eq!(messages[0].token_count, Some(5));
}

// ─── Token counter affects chunking ─────────────────────────

#[test]
fn test_chunk_token_counts_use_counter() {
    let counter = WordCounter;
    let text = "one two three four five six seven eight nine ten";
    let config = ChunkingConfig::default();
    let chunks = semantic_memory::chunker::chunk_text(text, &config, &counter);
    // Single chunk since text is short
    assert_eq!(chunks.len(), 1);
    // Each chunk's token_count_estimate should reflect word count, not chars/4
    let expected = chunks[0].content.split_whitespace().count();
    assert_eq!(chunks[0].token_count_estimate, expected);
}

// ─── Auto-computed token count on add_message ───────────────

#[tokio::test]
async fn test_auto_token_count() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();

    let session = store.create_session("test").await.unwrap();
    // Add message with token_count = None
    store
        .add_message(&session, Role::User, "hello world testing", None, None)
        .await
        .unwrap();
    let messages = store.get_recent_messages(&session, 10).await.unwrap();
    // Should have auto-computed token count (19 chars / 4 ≈ 4), not None
    assert!(messages[0].token_count.is_some());
    assert!(messages[0].token_count.unwrap() > 0);
}
