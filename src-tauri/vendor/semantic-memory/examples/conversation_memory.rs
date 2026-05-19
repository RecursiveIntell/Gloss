//! Conversation memory example.
//!
//! Creates a session, stores message embeddings locally with MockEmbedder, retrieves messages
//! within a token budget, and runs conversation search.
//!
//! Run: `cargo run --example conversation_memory`

use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder, Role};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = MemoryConfig {
        base_dir: PathBuf::from("/tmp/semantic-memory-conversation"),
        ..Default::default()
    };

    // Use MockEmbedder so the default `add_message()` path can embed and index messages locally.
    let store = MemoryStore::open_with_embedder(config, Box::new(MockEmbedder::new(768)))?;

    // Create a session
    let session_id = store.create_session("repl").await?;
    println!("Created session: {}", session_id);

    // Simulate a conversation
    let exchanges = [
        (Role::System, "You are a helpful assistant.", 20),
        (Role::User, "What is Rust?", 10),
        (
            Role::Assistant,
            "Rust is a systems programming language focused on safety, speed, and concurrency.",
            30,
        ),
        (Role::User, "What makes it safe?", 12),
        (
            Role::Assistant,
            "Rust's ownership system prevents data races and memory errors at compile time, without needing a garbage collector.",
            40,
        ),
        (Role::User, "Can you show me an example?", 15),
        (
            Role::Assistant,
            "Sure! Here's a simple Rust program:\n\nfn main() {\n    let msg = String::from(\"Hello\");\n    println!(\"{}\", msg);\n}",
            50,
        ),
    ];

    println!("\nAdding {} messages...", exchanges.len());
    for (role, content, tokens) in &exchanges {
        store
            .add_message(&session_id, *role, content, Some(*tokens), None)
            .await?;
    }

    // Get all messages
    println!("\nAll messages:");
    let all = store.get_recent_messages(&session_id, 100).await?;
    for msg in &all {
        println!("  [{}] {}", msg.role.as_str(), truncate(&msg.content, 60));
    }

    // Get messages within token budget
    let budget = 100;
    println!("\nMessages within {} token budget:", budget);
    let budgeted = store
        .get_messages_within_budget(&session_id, budget)
        .await?;
    for msg in &budgeted {
        println!(
            "  [{}] ({} tokens) {}",
            msg.role.as_str(),
            msg.token_count.unwrap_or(0),
            truncate(&msg.content, 60)
        );
    }

    // Token count
    let total = store.session_token_count(&session_id).await?;
    println!("\nTotal session tokens: {}", total);

    // Search the conversation using the hybrid message search path.
    let session_scope = [session_id.as_str()];
    let hits = store
        .search_conversations(
            "memory errors at compile time",
            Some(3),
            Some(&session_scope),
        )
        .await?;
    println!("\nConversation search:");
    for hit in &hits {
        println!("  [score: {:.4}] {}", hit.score, truncate(&hit.content, 60));
    }

    // List sessions
    let sessions = store.list_sessions(10, 0).await?;
    println!("\nSessions:");
    for s in &sessions {
        println!(
            "  {} (channel: {}, messages: {}, updated: {})",
            s.id, s.channel, s.message_count, s.updated_at
        );
    }

    // Cleanup
    std::fs::remove_dir_all("/tmp/semantic-memory-conversation").ok();

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
