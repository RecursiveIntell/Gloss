//! Basic search example.
//!
//! Creates a store, adds 5 facts, and searches for them.
//! Requires Ollama running at localhost:11434 with `nomic-embed-text`.
//!
//! Run: `cargo run --example basic_search`

use semantic_memory::{GraphDirection, MemoryConfig, MemoryStore};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber_init();

    let config = MemoryConfig {
        base_dir: PathBuf::from("/tmp/semantic-memory-example"),
        ..Default::default()
    };

    let store = MemoryStore::open(config)?;

    // Add some facts
    println!("Adding facts...");
    store
        .add_fact(
            "general",
            "Rust is a systems programming language focused on safety and performance",
            None,
            None,
        )
        .await?;
    store
        .add_fact(
            "general",
            "Python is widely used for data science and machine learning",
            None,
            None,
        )
        .await?;
    store
        .add_fact(
            "general",
            "JavaScript is the language of the web, running in all browsers",
            None,
            None,
        )
        .await?;
    store
        .add_fact(
            "user",
            "Josh prefers Rust for systems work and Python for scripting",
            None,
            None,
        )
        .await?;
    store
        .add_fact(
            "user",
            "The homelab runs NixOS on three servers",
            None,
            None,
        )
        .await?;

    // Search
    println!("\nSearching for 'systems programming'...");
    let results = store
        .search("systems programming", Some(3), None, None)
        .await?;
    for (i, result) in results.iter().enumerate() {
        println!(
            "  {}. [score: {:.4}] {}",
            i + 1,
            result.score,
            result.content
        );
    }

    // Explainable search exposes the exact live pipeline math.
    println!("\nExplainable search for 'systems programming'...");
    let explained = store
        .search_explained("systems programming", Some(3), None, None)
        .await?;
    for (i, hit) in explained.iter().enumerate() {
        println!(
            "  {}. [rrf: {:.4}] bm25_rank={:?} vector_rank={:?} reranked={} | {}",
            i + 1,
            hit.breakdown.rrf_score,
            hit.breakdown.bm25_rank,
            hit.breakdown.vector_rank,
            hit.breakdown.vector_reranked_from_f32,
            hit.result.content
        );
    }

    // FTS-only search (no Ollama needed)
    println!("\nFTS search for 'Python'...");
    let results = store.search_fts_only("Python", Some(3), None, None).await?;
    for (i, result) in results.iter().enumerate() {
        println!(
            "  {}. [score: {:.4}] {}",
            i + 1,
            result.score,
            result.content
        );
    }

    // Stats
    let stats = store.stats().await?;
    println!("\nDatabase stats:");
    println!("  Facts: {}", stats.total_facts);
    println!("  Documents: {}", stats.total_documents);
    println!("  Model: {:?}", stats.embedding_model);

    // Graph traversal is derived from SQLite state.
    let graph = store.graph_view();
    let namespace_edges = graph.neighbors("namespace:general", GraphDirection::Outgoing, 1)?;
    println!("\nGraph edges from namespace:general:");
    for edge in namespace_edges.iter().take(3) {
        println!("  {} -> {}", edge.source, edge.target);
    }

    // Cleanup
    std::fs::remove_dir_all("/tmp/semantic-memory-example").ok();

    Ok(())
}

fn tracing_subscriber_init() {
    // Simple stderr logger if tracing-subscriber is available
    // For the example, we just use eprintln
    let _ = std::io::stderr();
}
