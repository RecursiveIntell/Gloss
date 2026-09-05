//! Background calls receive a current, bounded grant; queue payloads carry
//! identity for comparison and never grant authority by themselves.
use super::{
    lan_local_providers_allowed, provider_row, validate_provider_base_url,
    validated_provider_base_url, ProviderConfig, ProviderType,
};
use crate::db::app_db::{AppDb, ModelRecord};
use crate::error::GlossError;

pub fn model_has_vision_capability(model: &ModelRecord) -> bool {
    if model.capabilities.as_deref().is_some_and(|capabilities| {
        capabilities.split(',').any(|capability| {
            matches!(
                capability.trim().to_ascii_lowercase().as_str(),
                "vision" | "image" | "multimodal"
            )
        })
    }) {
        return true;
    }
    let fingerprint = format!(
        "{} {}",
        model.id.to_ascii_lowercase(),
        model.display_name.to_ascii_lowercase()
    );
    [
        "llava",
        "bakllava",
        "moondream",
        "minicpm-v",
        "qwen-vl",
        "qwen2-vl",
        "qwen2.5-vl",
        "gemma3",
        "gemma4",
        "vision",
        "vl",
    ]
    .iter()
    .any(|needle| fingerprint.contains(needle))
}

pub fn validate_background_dispatch(
    app_db: &AppDb,
    model_setting_key: &str,
    expected_url: &str,
    expected_model: &str,
) -> Result<ProviderConfig, GlossError> {
    if !matches!(model_setting_key, "summary_model" | "vision_model") {
        return Err(GlossError::Config(
            "Unknown background model purpose".into(),
        ));
    }
    let configured_model = app_db
        .get_setting(model_setting_key)?
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty());
    if configured_model.is_none()
        && app_db.get_setting("default_provider")?.as_deref() != Some("ollama")
    {
        return Err(GlossError::Config(
            "Background jobs require an Ollama model".into(),
        ));
    }
    let selected_model = configured_model
        .or(app_db
            .get_setting("default_model")?
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty()))
        .ok_or_else(|| GlossError::Config("No background model is configured".into()))?;
    if expected_model != selected_model {
        return Err(GlossError::Config(
            "Background model changed since enqueue; retry required".into(),
        ));
    }
    let models = app_db.get_all_models()?;
    let model = models
        .iter()
        .find(|model| {
            model.id == selected_model
                && model.provider_id == "ollama"
                && model.available
                && !model.stale
        })
        .ok_or_else(|| {
            GlossError::Config("Background Ollama model is unavailable or stale".into())
        })?;
    if model_setting_key == "vision_model" && !model_has_vision_capability(model) {
        return Err(GlossError::Config(
            "Background vision requires a vision-capable Ollama model".into(),
        ));
    }
    let providers = app_db.list_providers()?;
    let row = provider_row(&providers, ProviderType::Ollama)
        .filter(|row| row.enabled)
        .ok_or_else(|| {
            GlossError::Config("Background Ollama provider is disabled or missing".into())
        })?;
    let allow_lan = lan_local_providers_allowed(app_db);
    let base_url = validated_provider_base_url(row, ProviderType::Ollama, allow_lan, false)?;
    let expected =
        validate_provider_base_url(ProviderType::Ollama, expected_url, allow_lan, false)?;
    if base_url != expected.base_url {
        return Err(GlossError::Config(
            "Background endpoint changed since enqueue; retry required".into(),
        ));
    }
    Ok(ProviderConfig {
        provider_type: ProviderType::Ollama,
        base_url,
        api_key: None,
    })
}

/// Dispatch entrypoint shared by all persisted provider-backed job variants.
/// The provider is created only after current settings authorize this identity.
pub fn background_provider_for_dispatch(
    data_dir: &std::path::Path,
    model_setting_key: &str,
    expected_url: &str,
    expected_model: &str,
) -> Result<Box<dyn super::LlmProvider>, GlossError> {
    let app_db = AppDb::open(&data_dir.join("gloss.db"))?;
    let snapshot = app_db.conn().unchecked_transaction()?;
    let config =
        validate_background_dispatch(&app_db, model_setting_key, expected_url, expected_model)?;
    snapshot.commit()?;
    super::build_provider(&config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tokio::time::{timeout, Duration};

    fn setup(dir: &std::path::Path, url: &str) -> AppDb {
        let db = AppDb::open(&dir.join("gloss.db")).unwrap();
        db.update_provider("ollama", true, Some(url)).unwrap();
        db.set_setting("summary_model", "fixture-model").unwrap();
        db.set_setting("vision_model", "fixture-model").unwrap();
        db.replace_models(
            "ollama",
            &[ModelRecord {
                id: "fixture-model".into(),
                provider_id: "ollama".into(),
                display_name: "Fixture".into(),
                parameter_size: None,
                context_window: None,
                capabilities: Some("vision".into()),
                available: true,
                stale: false,
                last_error: None,
            }],
        )
        .unwrap();
        db
    }

    #[tokio::test]
    async fn dispatch_rechecks_disable_endpoint_model_and_availability_before_network() {
        for setting in ["summary_model", "vision_model"] {
            for change in ["disable", "endpoint", "model", "stale"] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let dir = tempfile::tempdir().unwrap();
                let db = setup(dir.path(), &url);
                validate_background_dispatch(&db, setting, &url, "fixture-model").unwrap();
                match change {
                    "disable" => db.update_provider("ollama", false, None).unwrap(),
                    "endpoint" => db
                        .update_provider("ollama", true, Some("http://127.0.0.1:1"))
                        .unwrap(),
                    "model" => db.set_setting(setting, "replacement-model").unwrap(),
                    "stale" => db
                        .mark_models_unavailable("ollama", "fixture unavailable")
                        .unwrap(),
                    _ => unreachable!(),
                }
                assert!(
                    background_provider_for_dispatch(dir.path(), setting, &url, "fixture-model")
                        .is_err(),
                    "{setting}/{change} must revoke dispatch"
                );
                assert!(
                    timeout(Duration::from_millis(20), listener.accept())
                        .await
                        .is_err(),
                    "revoked endpoint received a connection"
                );
            }
        }
    }

    #[test]
    fn dispatch_rechecks_lan_opt_in_and_vision_capability() {
        let dir = tempfile::tempdir().unwrap();
        let url = "http://192.168.1.4:11434";
        let db = setup(dir.path(), url);
        db.set_setting("allow_lan_local_providers", "true").unwrap();
        for setting in ["summary_model", "vision_model"] {
            validate_background_dispatch(&db, setting, url, "fixture-model").unwrap();
        }
        db.set_setting("allow_lan_local_providers", "false")
            .unwrap();
        for setting in ["summary_model", "vision_model"] {
            assert!(
                background_provider_for_dispatch(dir.path(), setting, url, "fixture-model")
                    .is_err()
            );
        }
        db.set_setting("allow_lan_local_providers", "true").unwrap();
        db.conn()
            .execute("UPDATE models SET capabilities = NULL", [])
            .unwrap();
        assert!(validate_background_dispatch(&db, "vision_model", url, "fixture-model").is_err());
        assert!(validate_background_dispatch(&db, "summary_model", url, "fixture-model").is_ok());
    }

    #[tokio::test]
    async fn unchanged_dispatch_uses_actual_provider_and_preserves_identity() {
        for setting in ["summary_model", "vision_model"] {
            let body = br#"{"message":{"content":"authorized"},"done":true}"#.to_vec();
            let (url, fixture) = super::super::test_http::respond("200 OK", body, "").await;
            let dir = tempfile::tempdir().unwrap();
            let _db = setup(dir.path(), &url);
            let provider =
                background_provider_for_dispatch(dir.path(), setting, &url, "fixture-model")
                    .unwrap();
            let request = super::super::ChatRequest {
                model: "fixture-model".into(),
                system_prompt: None,
                messages: vec![super::super::ChatMessage {
                    role: "user".into(),
                    content: "source text".into(),
                    images: None,
                }],
                max_tokens: 10,
                temperature: 0.0,
                top_p: None,
                top_k: None,
                min_p: None,
                repeat_penalty: None,
                stream: false,
                num_ctx: None,
            };
            let mut stream = provider
                .chat(request, super::super::LlmExecutionContext::uncancellable())
                .await
                .unwrap();
            assert_eq!(stream.next().await.unwrap().unwrap().token, "authorized");
            let request = fixture.await.unwrap();
            let request = String::from_utf8_lossy(&request);
            assert!(request.contains("fixture-model"));
            assert!(request.contains("source text"));
        }
    }
}
