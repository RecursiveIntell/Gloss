use crate::db;
use crate::error::MemoryError;
use crate::knowledge;
use crate::projection_import;
use crate::quantize;
use crate::MemoryStore;

fn serialize_import_json<T: serde::Serialize>(
    field: &'static str,
    value: &T,
) -> Result<String, MemoryError> {
    serde_json::to_string(value).map_err(|err| MemoryError::ImportInvalid {
        reason: format!("failed to serialize legacy episode {field}: {err}"),
    })
}

pub(crate) async fn import_envelope(
    store: &MemoryStore,
    envelope: &projection_import::ImportEnvelope,
) -> Result<projection_import::ImportReceipt, MemoryError> {
    envelope.validate()?;

    let (eid, sv, cd) = envelope.dedupe_key();
    let eid = eid.to_string();
    let sv = sv.to_string();
    let cd = cd.to_string();

    let already = {
        let eid_c = eid.clone();
        let sv_c = sv.clone();
        let cd_c = cd.clone();
        store
            .with_read_conn(move |conn| {
                projection_import::check_import_exists(conn, &eid_c, &sv_c, &cd_c)
            })
            .await?
    };
    if already {
        return Ok(projection_import::ImportReceipt {
            envelope_id: envelope.envelope_id.clone(),
            schema_version: envelope.schema_version.clone(),
            content_digest: envelope.content_digest.clone(),
            status: projection_import::ImportStatus::AlreadyImported,
            record_count: envelope.records.len(),
            imported_at: String::new(),
            was_duplicate: true,
            trace_id: envelope.trace_id.clone(),
        });
    }

    let mut prepared = Vec::new();
    for (i, record) in envelope.records.iter().enumerate() {
        let embedding = store.embed_text_internal(record.content_text()).await?;
        store.validate_embedding_dimensions(&embedding)?;
        let embedding_bytes = db::embedding_to_bytes(&embedding);
        let q8_bytes = quantize::Quantizer::new(store.inner.config.embedding.dimensions)
            .quantize(&embedding)
            .map(|qv| quantize::pack_quantized(&qv))
            .ok();
        prepared.push((i, embedding_bytes, q8_bytes));
    }

    let envelope_c = envelope.clone();
    let record_count = envelope.records.len();
    let receipt = store
        .with_write_conn(move |conn| {
            db::with_transaction(conn, |tx| {
                if projection_import::check_import_exists(
                    tx,
                    envelope_c.envelope_id.as_str(),
                    &envelope_c.schema_version,
                    &envelope_c.content_digest,
                )? {
                    return Ok(projection_import::ImportReceipt {
                        envelope_id: envelope_c.envelope_id.clone(),
                        schema_version: envelope_c.schema_version.clone(),
                        content_digest: envelope_c.content_digest.clone(),
                        status: projection_import::ImportStatus::AlreadyImported,
                        record_count,
                        imported_at: String::new(),
                        was_duplicate: true,
                        trace_id: envelope_c.trace_id.clone(),
                    });
                }

                for (i, embedding_bytes, q8_bytes) in &prepared {
                    let record = &envelope_c.records[*i];
                    let provenance_meta = serde_json::json!({
                        "import_envelope_id": envelope_c.envelope_id.as_str(),
                        "import_source_authority": envelope_c.source_authority,
                        "import_schema_version": envelope_c.schema_version,
                    });

                    match record {
                        projection_import::ImportRecord::Fact {
                            content,
                            source,
                            metadata,
                        } => {
                            let fact_id = uuid::Uuid::new_v4().to_string();
                            let mut meta = metadata.clone().unwrap_or(serde_json::json!({}));
                            if let Some(obj) = meta.as_object_mut() {
                                obj.insert("_import".into(), provenance_meta.clone());
                            }
                            if let Some(trace_id) = &envelope_c.trace_id {
                                if let Some(obj) = meta.as_object_mut() {
                                    obj.insert(
                                        "trace_id".into(),
                                        serde_json::Value::String(trace_id.0.clone()),
                                    );
                                }
                            }

                            knowledge::insert_fact_in_tx(
                                tx,
                                &fact_id,
                                &envelope_c.namespace,
                                content,
                                embedding_bytes,
                                q8_bytes.as_deref(),
                                source.as_deref(),
                                Some(&meta),
                            )?;
                        }
                        projection_import::ImportRecord::Episode {
                            document_id,
                            meta,
                        } => {
                            let episode_id = uuid::Uuid::new_v4().to_string();
                            let meta_json = serde_json::to_value(meta).map_err(|e| {
                                MemoryError::ImportInvalid {
                                    reason: format!("failed to serialize episode meta: {e}"),
                                }
                            })?;
                            let cause_ids_json =
                                serialize_import_json("cause_ids", &meta.cause_ids)?;
                            let verification_json = serialize_import_json(
                                "verification_status",
                                &meta.verification_status,
                            )?;
                            let search_text = format!(
                                "{} {} {} {}",
                                meta.effect_type,
                                meta.outcome.as_str(),
                                meta.experiment_id.as_deref().unwrap_or(""),
                                cause_ids_json
                            );
                            let _ = meta_json;

                            tx.execute(
                                "INSERT INTO episodes
                                 (episode_id, document_id, cause_ids, effect_type, outcome,
                                  confidence, verification_status, experiment_id,
                                  search_text, embedding, embedding_q8, trace_id)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                                rusqlite::params![
                                    episode_id,
                                    document_id,
                                    cause_ids_json,
                                    meta.effect_type,
                                    meta.outcome.as_str(),
                                    meta.confidence,
                                    verification_json,
                                    meta.experiment_id,
                                    search_text,
                                    embedding_bytes,
                                    q8_bytes.as_deref(),
                                    envelope_c
                                        .trace_id
                                        .as_ref()
                                        .map(|t| t.as_str().to_string()),
                                ],
                            )?;

                            tx.execute(
                                "INSERT INTO episodes_rowid_map (episode_id, document_id) VALUES (?1, ?2)",
                                rusqlite::params![episode_id, document_id],
                            )?;
                            let fts_rowid = tx.last_insert_rowid();
                            tx.execute(
                                "INSERT INTO episodes_fts(rowid, content) VALUES (?1, ?2)",
                                rusqlite::params![fts_rowid, search_text],
                            )?;

                            for (ordinal, cause_id) in meta.cause_ids.iter().enumerate() {
                                tx.execute(
                                    "INSERT OR IGNORE INTO episode_causes (episode_id, cause_node_id, ordinal)
                                     VALUES (?1, ?2, ?3)",
                                    rusqlite::params![episode_id, cause_id, ordinal as i64],
                                )?;
                            }

                            #[cfg(feature = "hnsw")]
                            db::enqueue_pending_index_op(
                                tx,
                                &format!("episode:{}", episode_id),
                                "episode",
                                db::PendingIndexOpKind::Upsert,
                            )?;
                        }
                    }
                }

                projection_import::insert_import_log(
                    tx,
                    &envelope_c,
                    &projection_import::ImportStatus::Complete,
                    record_count,
                )?;

                Ok(projection_import::ImportReceipt {
                    envelope_id: envelope_c.envelope_id.clone(),
                    schema_version: envelope_c.schema_version.clone(),
                    content_digest: envelope_c.content_digest.clone(),
                    status: projection_import::ImportStatus::Complete,
                    record_count,
                    imported_at: chrono::Utc::now().to_rfc3339(),
                    was_duplicate: false,
                    trace_id: envelope_c.trace_id.clone(),
                })
            })
        })
        .await?;

    #[cfg(feature = "hnsw")]
    store
        .sync_pending_hnsw_ops_best_effort("import_envelope")
        .await;

    Ok(receipt)
}

pub(crate) async fn import_status(
    store: &MemoryStore,
    envelope_id: &projection_import::EnvelopeId,
) -> Result<Vec<projection_import::ImportReceipt>, MemoryError> {
    let eid = envelope_id.0.clone();
    store
        .with_read_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT envelope_id, schema_version, content_digest, status,
                        record_count, imported_at, trace_id
                 FROM import_log
                 WHERE envelope_id = ?1
                 ORDER BY imported_at DESC",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![eid], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(rows
                .into_iter()
                .map(|(eid, sv, cd, status, rc, ts, tid)| {
                    let status_parsed = projection_import::ImportStatus::from_str_value(&status);
                    let was_dup = matches!(
                        status_parsed,
                        projection_import::ImportStatus::AlreadyImported
                    );
                    projection_import::ImportReceipt {
                        envelope_id: projection_import::EnvelopeId(eid),
                        schema_version: sv,
                        content_digest: cd,
                        status: status_parsed,
                        record_count: rc as usize,
                        imported_at: ts,
                        was_duplicate: was_dup,
                        trace_id: tid.map(crate::types::TraceId::new),
                    }
                })
                .collect())
        })
        .await
}

pub(crate) async fn list_imports(
    store: &MemoryStore,
    namespace: Option<&str>,
    limit: usize,
) -> Result<Vec<projection_import::ImportReceipt>, MemoryError> {
    let ns = namespace.map(str::to_string);
    store
        .with_read_conn(move |conn| projection_import::query_import_log(conn, ns.as_deref(), limit))
        .await
}

pub(crate) async fn last_import_at(
    store: &MemoryStore,
    namespace: &str,
) -> Result<Option<String>, MemoryError> {
    let ns = namespace.to_string();
    store
        .with_read_conn(move |conn| projection_import::last_import_at(conn, &ns))
        .await
}
