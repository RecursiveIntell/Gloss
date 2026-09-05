//! Disposable file -> real chunker -> real HTTP embedding transport -> real
//! SQLite/native publication -> cold reload proof. Tauri orchestration and
//! model quality are outside this fixture's claim.
use gloss_native_contract_tests::{
    chunk, dense, embedding_contract,
    notebook_db::{Chunk, EmbeddingIndexMetadata, NotebookDb, NATIVE_HNSW_INDEX_ID},
};
use std::io::{Read, Write};

#[test]
fn short_code_and_toml_files_recover_into_a_searchable_notebook() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("notebook.db");
    let db = NotebookDb::open(&db_path).unwrap();
    let inputs = [
        ("Cargo.toml", "name = \"gloss\"\n#notebook".to_string()),
        ("lib.rs", "hello".to_string()),
        (
            "config.rs",
            (0..141)
                .map(|i| format!("word{i}"))
                .collect::<Vec<_>>()
                .join(" "),
        ),
    ];
    let mut expected_texts = Vec::new();
    for (name, text) in &inputs {
        let file = temp.path().join(name);
        std::fs::write(&file, text).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        db.conn()
            .execute(
                "INSERT INTO sources(id,source_type,title,status,error_message) VALUES(?1,'code',?1,'error','write connection not available')",
                [name],
            )
            .unwrap();
        assert!(db.get_chunks_for_source(name).unwrap().is_empty());
        assert_eq!(
            db.reset_source_for_reingestion("fixture-notebook", name)
                .unwrap(),
            "code"
        );
        let chunks = chunk::chunk_text_with_title(&text, name, name, Some(1100));
        assert!(
            !chunks.is_empty(),
            "short file must remain indexable: {name}"
        );
        expected_texts.extend(chunks.iter().map(|chunk| chunk.content.clone()));
        db.insert_chunks(
            &chunks
                .into_iter()
                .map(|chunk| Chunk {
                    id: chunk.id,
                    source_id: name.to_string(),
                    chunk_index: chunk.chunk_index,
                    content: chunk.content,
                    token_count: chunk.token_count,
                    start_offset: chunk.start_offset,
                    end_offset: chunk.end_offset,
                    metadata: chunk.metadata,
                    embedding_id: None,
                    embedding_model: None,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    let initial = db.native_rebuild_chunks().unwrap();
    let metadata = EmbeddingIndexMetadata::ready(
        NATIVE_HNSW_INDEX_ID,
        "fixture",
        "fixture",
        Some("fixture-only".into()),
        3,
    );
    let broken = dense::begin_dense_rebuild(&db, &metadata).unwrap();
    assert!(broken
        .build(
            |_| Err(gloss_native_contract_tests::error::GlossError::Embedding(
                "injected provider unavailable".into()
            ))
        )
        .is_err());
    broken.fail(&db, "injected provider unavailable").unwrap();
    assert_eq!(db.native_rebuild_chunks().unwrap(), initial);
    for (name, _) in &inputs {
        db.update_source_index_status(
            name,
            None,
            Some("blocked"),
            Some("native provider unavailable"),
        )
        .unwrap();
        db.update_source_status(name, "error", Some("native provider unavailable"))
            .unwrap();
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let fixture = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        let (header_end, body_len) = loop {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let len = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (end + 4, len);
            }
        };
        while request.len() < header_end + body_len {
            let count = socket.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        let body: serde_json::Value =
            serde_json::from_slice(&request[header_end..header_end + body_len]).unwrap();
        let texts = body["input"].as_array().unwrap();
        let vectors = texts
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let mut vector = vec![0.0f32; 3];
                vector[i % 3] = 1.0;
                vector
            })
            .collect::<Vec<_>>();
        let response = serde_json::json!({"embeddings":vectors}).to_string();
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        texts
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    });
    let client = embedding_contract::ollama_client(&url, "fixture", 5, false).unwrap();
    let rebuild = dense::begin_dense_rebuild(&db, &metadata).unwrap();
    let candidate = rebuild
        .build(|chunks| {
            embedding_contract::ollama_embed_sync(
                &client,
                &url,
                "fixture",
                &chunks
                    .iter()
                    .map(|chunk| chunk.content.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap();
    let path = dense::native_dense_artifact_path(temp.path());
    let receipt = rebuild.publish(&db, &path, &candidate).unwrap();
    assert_eq!(receipt.chunks_indexed, expected_texts.len());
    assert_eq!(receipt.sources_recovered, 3);
    let mut sent = fixture.join().unwrap();
    sent.sort();
    expected_texts.sort();
    assert_eq!(sent, expected_texts);
    drop(db);
    let db = NotebookDb::connect(&db_path).unwrap();
    let index = dense::load_published_dense_index(&db, &path, &metadata).unwrap();
    for (label, _) in index.search(&[1.0, 0.0, 0.0], 3).unwrap() {
        assert!(!db
            .get_chunk_by_embedding_id(label as i64)
            .unwrap()
            .content
            .is_empty());
    }
    assert_eq!(db.native_embedding_ids().unwrap().len(), 3);
    for (name, _) in inputs {
        let source = db.get_source(name).unwrap();
        assert_eq!(source.status, "ready");
        assert_eq!(
            source.processing_state.unwrap().dense_index_status,
            "indexed"
        );
    }
}
