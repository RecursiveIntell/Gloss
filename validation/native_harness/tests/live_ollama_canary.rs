//! Real downloaded-model canary. Ordinary contract tests never run this test.
//! The dedicated hosted CI job supplies an isolated loopback Ollama service.
//! This certifies these tiny public models and owners, not user model quality,
//! installed desktop state, Tauri orchestration, or the Candle backend.
use futures::StreamExt;
use gloss_native_contract_tests::{
    dense, embedding_contract,
    notebook_db::{Chunk, EmbeddingIndexMetadata, NotebookDb, NATIVE_HNSW_INDEX_ID},
    providers::{self, ChatMessage, ChatRequest, LlmExecutionContext, LlmProvider},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const URL: &str = "http://127.0.0.1:11435";
const EMBED_MODEL: &str = "all-minilm:22m";
const CHAT_MODEL: &str = "qwen3:0.6b";

fn request() -> ChatRequest {
    ChatRequest {
        model: CHAT_MODEL.into(),
        system_prompt: Some("Give a short direct answer.".into()),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "Say hello in one short sentence. /no_think".into(),
            images: None,
        }],
        max_tokens: 128,
        temperature: 0.0,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        stream: true,
        num_ctx: Some(1024),
    }
}

#[test]
#[ignore = "requires explicit isolated real Ollama canary setup; never substitute HTTP fixtures"]
fn live_ollama_embed_publish_reload_chat_and_precancel() {
    assert_eq!(
        std::env::var("GLOSS_LIVE_OLLAMA_CANARY").as_deref(),
        Ok("1"),
        "--ignored is insufficient: explicit canary authorization is required"
    );
    assert_eq!(
        std::env::var("OLLAMA_HOST").as_deref(),
        Ok("127.0.0.1:11435")
    );
    let output =
        PathBuf::from(std::env::var("GLOSS_LIVE_OLLAMA_RECEIPT").expect("receipt path required"));
    assert!(!output.exists(), "refuse stale live receipt");
    let embed_digest =
        std::env::var("GLOSS_LIVE_EMBED_DIGEST").expect("verified model digest required");
    let chat_digest =
        std::env::var("GLOSS_LIVE_CHAT_DIGEST").expect("verified model digest required");
    assert!(embed_digest.starts_with("1b226e2802db"));
    assert!(chat_digest.starts_with("7df6b6e09427"));
    let started = Instant::now();
    let texts = [
        "The orchard grows apples and pears in the warm summer sunlight.",
        "A submarine explores the dark ocean floor beneath the cold waves.",
    ];
    let client = embedding_contract::ollama_client(URL, EMBED_MODEL, 120, false).unwrap();
    let vectors = embedding_contract::ollama_embed_sync(&client, URL, EMBED_MODEL, &texts).unwrap();
    assert_eq!(vectors.len(), 2);
    let dimensions = vectors[0].len();
    assert!(dimensions > 0);
    for vector in &vectors {
        assert_eq!(vector.len(), dimensions);
        assert!(vector.iter().all(|value| value.is_finite()));
        assert!(
            vector.iter().any(|value| *value != 0.0),
            "zero embedding is not a live success"
        );
    }
    assert_ne!(
        vectors[0], vectors[1],
        "distinct texts must not produce identical embeddings"
    );

    let native = output.parent().unwrap().join("native");
    std::fs::create_dir_all(&native).unwrap();
    let db_path = native.join("notebook.db");
    assert!(
        !db_path.exists(),
        "canary notebook must be disposable and new"
    );
    let db = NotebookDb::open(&db_path).unwrap();
    db.conn().execute("INSERT INTO sources(id,source_type,title,status) VALUES('canary','text','Synthetic canary','ready')", []).unwrap();
    let chunks = texts
        .iter()
        .enumerate()
        .map(|(index, text)| Chunk {
            id: format!("canary-{index}"),
            source_id: "canary".into(),
            chunk_index: index as i32,
            content: text.to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        })
        .collect::<Vec<_>>();
    db.insert_chunks(&chunks).unwrap();
    let metadata = EmbeddingIndexMetadata::ready(
        NATIVE_HNSW_INDEX_ID,
        "ollama",
        EMBED_MODEL,
        Some(embed_digest.clone()),
        dimensions,
    );
    let artifact = dense::native_dense_artifact_path(&native);
    assert_eq!(
        dense::publish_dense_batch(&db, &artifact, &chunks, &vectors, &metadata).unwrap(),
        2
    );
    drop(db);

    // Cold SQLite connection and disk-loaded native index; no cached index reuse.
    let reopened = NotebookDb::connect(&db_path).unwrap();
    let cold_index = dense::load_published_dense_index(&reopened, &artifact, &metadata).unwrap();
    assert_eq!(cold_index.size(), 2);
    let query =
        embedding_contract::ollama_embed_sync(&client, URL, EMBED_MODEL, &[texts[0]]).unwrap();
    let matches = cold_index.search(&query[0], 2).unwrap();
    assert_eq!(matches.len(), 2);
    let winner = reopened
        .get_chunk_by_embedding_id(matches[0].0 as i64)
        .unwrap();
    assert_eq!(
        winner.id, "canary-0",
        "cold retrieval must recover the matching canonical text"
    );
    assert_eq!(winner.content, texts[0]);
    assert!(matches.iter().all(|(_, distance)| distance.is_finite()));
    drop(cold_index);
    drop(reopened);
    drop(client);

    // Chat starts only after embedding and native publication finish.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (answer, tokens, precancel_ms) = runtime.block_on(async {
        let provider =
            providers::ollama::OllamaProvider::new(URL, providers::build_shared_client().unwrap());
        let chat = async {
            let mut stream = provider
                .chat(request(), LlmExecutionContext::uncancellable())
                .await
                .unwrap();
            let mut answer = String::new();
            let mut tokens = 0usize;
            let mut terminal = false;
            while let Some(token) = stream.next().await {
                let token = token.unwrap();
                answer.push_str(&token.token);
                tokens += 1;
                assert!(answer.len() <= 16 * 1024, "bounded canary output");
                if token.done {
                    terminal = true;
                    break;
                }
            }
            assert!(terminal, "real Ollama must supply its terminal marker");
            assert!(
                !answer.trim().is_empty(),
                "real model answer must be nonempty"
            );
            assert!(
                stream.next().await.is_none(),
                "terminal provider stream must be fused"
            );
            (answer, tokens)
        };
        let (answer, tokens) = tokio::time::timeout(Duration::from_secs(180), chat)
            .await
            .expect("real chat deadline");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let before = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            provider.chat(
                request(),
                LlmExecutionContext::default_with_token(cancellation),
            ),
        )
        .await
        .expect("pre-cancel must return promptly");
        let error = match result {
            Ok(_) => panic!("pre-cancelled call must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cancelled"));
        (answer, tokens, before.elapsed().as_millis())
    });
    let receipt = json!({
        "schema": "GlossLiveOllamaOwnerCanaryV1", "status": "pass",
        "real_service_exercised": true, "http_fixture_used": false,
        "provider": "ollama", "endpoint": URL,
        "embedding_model": EMBED_MODEL, "embedding_model_digest": embed_digest,
        "chat_model": CHAT_MODEL, "chat_model_digest": chat_digest,
        "embedding_count": 2, "dimensions": dimensions,
        "native_published_chunks": 2, "cold_search_winner": winner.id,
        "artifact_sha256": format!("{:x}", Sha256::digest(std::fs::read(&artifact).unwrap())),
        "chat_terminal": true, "chat_nonempty": true, "chat_tokens": tokens,
        "chat_answer": answer, "precancel_rejected": true, "precancel_ms": precancel_ms,
        "elapsed_ms": started.elapsed().as_millis(),
        "coverage_limits": ["Synthetic public canary data only", "No user-installed model, GUI, Tauri job orchestration, or Candle claim"]
    });
    std::fs::write(&output, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();
    println!("{}", serde_json::to_string(&receipt).unwrap());
}
