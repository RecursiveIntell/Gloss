//! Validation and atomic persistence for operator settings.
use crate::{db::app_db::AppDb, error::GlossError, providers};
use serde::Deserialize;

pub const DEFAULT_SEMANTIC_SEARCH_TIMEOUT_MS: u64 = 60_000;
pub const MAX_SEMANTIC_SEARCH_TIMEOUT_MS: u64 = 300_000;
const SEMANTIC_SEARCH_TIMEOUT_GRACE_MS: u64 = 5_000;

pub fn minimum_semantic_search_timeout_ms(embedding_timeout_secs: u64) -> u64 {
    embedding_timeout_secs
        .saturating_mul(1_000)
        .saturating_add(SEMANTIC_SEARCH_TIMEOUT_GRACE_MS)
        .min(MAX_SEMANTIC_SEARCH_TIMEOUT_MS)
}

pub fn proven_semantic_search_timeout_ms(
    configured_timeout_ms: u64,
    embedding_timeout_secs: u64,
    retrieval_probe_elapsed_ms: u64,
    turbo_quant_requested: bool,
) -> u64 {
    let proof_budget = retrieval_probe_elapsed_ms
        .saturating_mul(2)
        .saturating_add(SEMANTIC_SEARCH_TIMEOUT_GRACE_MS);
    let turbo_quant_floor = if turbo_quant_requested {
        DEFAULT_SEMANTIC_SEARCH_TIMEOUT_MS
    } else {
        0
    };
    configured_timeout_ms
        .max(minimum_semantic_search_timeout_ms(embedding_timeout_secs))
        .max(proof_budget)
        .max(turbo_quant_floor)
        .min(MAX_SEMANTIC_SEARCH_TIMEOUT_MS)
}

pub fn invalidate_existing_embedding_indexes(
    db: &crate::db::notebook_db::NotebookDb,
) -> Result<(), GlossError> {
    use crate::db::notebook_db::{
        EmbeddingIndexMetadataStatus, NATIVE_HNSW_INDEX_ID, SEMANTIC_MEMORY_INDEX_ID,
    };
    for index_id in [NATIVE_HNSW_INDEX_ID, SEMANTIC_MEMORY_INDEX_ID] {
        if db.embedding_index_metadata(index_id)?.is_some() {
            db.mark_embedding_index_status(index_id, EmbeddingIndexMetadataStatus::Stale,
                Some("embedding-index-stale: embedding configuration changed; rebuild notebook index"))?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingSettings {
    pub provider: String,
    pub url: String,
    pub model: String,
    pub timeout_secs: u64,
    pub download_consent: bool,
    pub search_timeout_ms: u64,
    pub chunk_target_tokens: u64,
}

pub fn save_embedding_settings(db: &AppDb, config: &EmbeddingSettings) -> Result<bool, GlossError> {
    validate_setting_value("semantic_memory_embedding_provider", &config.provider)?;
    validate_setting_value(
        "semantic_memory_embedding_timeout_secs",
        &config.timeout_secs.to_string(),
    )?;
    if config.provider == "ollama" {
        validate_setting_value("semantic_memory_embedding_model", &config.model)?;
        providers::validate_embedding_url(&config.url, providers::lan_local_providers_allowed(db))?;
    }
    validate_setting_value(
        "semantic_memory_search_timeout_ms",
        &config.search_timeout_ms.to_string(),
    )?;
    if config.provider == "ollama" {
        let minimum = minimum_semantic_search_timeout_ms(config.timeout_secs);
        if config.search_timeout_ms < minimum {
            return Err(GlossError::Config(format!(
                "semantic_memory_search_timeout_ms must allow embedding timeout plus retrieval overhead (minimum {minimum} ms for {} s embedding timeout)",
                config.timeout_secs
            )));
        }
    }
    validate_setting_value(
        "chunk_target_tokens",
        &config.chunk_target_tokens.to_string(),
    )?;

    fn derivation_identity(
        provider: &str,
        url: Option<&str>,
        model: Option<&str>,
    ) -> (String, String, String) {
        match provider {
            "fastembed" | "native" => ("fastembed".to_string(), String::new(), String::new()),
            _ => (
                provider.to_string(),
                url.unwrap_or_default()
                    .trim()
                    .trim_end_matches('/')
                    .to_string(),
                model.unwrap_or_default().trim().to_string(),
            ),
        }
    }

    let prior_provider = db.get_setting("semantic_memory_embedding_provider")?;
    let prior_identity = match prior_provider.as_deref() {
        Some(provider) => Some(derivation_identity(
            provider,
            db.get_setting("semantic_memory_embedding_url")?.as_deref(),
            db.get_setting("semantic_memory_embedding_model")?
                .as_deref(),
        )),
        None => None,
    };
    let next_identity =
        derivation_identity(&config.provider, Some(&config.url), Some(&config.model));
    let identity_changed = prior_identity.as_ref() != Some(&next_identity);
    let timeout = config.timeout_secs.to_string();
    let search_timeout = config.search_timeout_ms.to_string();
    let chunk_tokens = config.chunk_target_tokens.to_string();
    db.set_settings_atomically(&[
        ("semantic_memory_embedding_provider", &config.provider),
        ("semantic_memory_embedding_url", &config.url),
        ("semantic_memory_embedding_model", &config.model),
        ("semantic_memory_embedding_timeout_secs", &timeout),
        (
            "fastembed_download_consent",
            if config.download_consent {
                "true"
            } else {
                "false"
            },
        ),
        ("semantic_memory_search_timeout_ms", &search_timeout),
        ("chunk_target_tokens", &chunk_tokens),
    ])?;
    Ok(identity_changed)
}

pub fn validate_setting_value(key: &str, value: &str) -> Result<(), GlossError> {
    let invalid = || GlossError::Config(format!("Invalid value for {key}"));
    let integer = |min: u64, max: u64| -> Result<(), GlossError> {
        value
            .parse::<u64>()
            .ok()
            .filter(|value| (min..=max).contains(value))
            .map(|_| ())
            .ok_or_else(invalid)
    };
    let number = |min: f64, max: f64| -> Result<(), GlossError> {
        value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && (min..=max).contains(value))
            .map(|_| ())
            .ok_or_else(invalid)
    };
    match key {
        "semantic_memory_embedding_provider"
            if matches!(value, "ollama" | "fastembed" | "native") =>
        {
            Ok(())
        }
        "semantic_memory_embedding_provider" => Err(invalid()),
        "semantic_memory_embedding_model"
            if !value.trim().is_empty() && value.trim() == value && value.len() <= 512 =>
        {
            Ok(())
        }
        "semantic_memory_embedding_model" => Err(invalid()),
        "semantic_memory_embedding_timeout_secs" => integer(2, 300),
        "semantic_memory_search_timeout_ms" => integer(100, 300_000),
        "chunk_target_tokens" => integer(100, 3000),
        "generation_temperature" => number(0.0, 2.0),
        "generation_top_p" | "generation_min_p" => {
            if value.is_empty() {
                Ok(())
            } else {
                number(0.0, 1.0)
            }
        }
        "generation_top_k" => {
            if value.is_empty() {
                Ok(())
            } else {
                integer(0, 100_000)
            }
        }
        "generation_repeat_penalty" => {
            if value.is_empty() {
                Ok(())
            } else {
                number(0.0, 10.0)
            }
        }
        "memory_backend" if matches!(value, "gloss-local" | "semantic-memory-preview") => Ok(()),
        "memory_backend" => Err(invalid()),
        "summary_mode" if matches!(value, "auto" | "manual") => Ok(()),
        "summary_mode" => Err(invalid()),
        "fastembed_download_consent"
        | "allow_lan_local_providers"
        | "allow_custom_cloud_endpoints"
        | "memory_backend_fallback"
        | "semantic_memory_auto_project"
        | "semantic_memory_strict_testing"
        | "semantic_memory_turbo_quant_require_fresh_artifacts"
        | "semantic_memory_provekv_pool_candidates_enabled" => {
            if matches!(value, "true" | "false") {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_configuration_does_not_mark_an_empty_notebook_stale() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::notebook_db::NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        invalidate_existing_embedding_indexes(&db).unwrap();
        assert!(db
            .embedding_index_metadata(crate::db::notebook_db::NATIVE_HNSW_INDEX_ID)
            .unwrap()
            .is_none());
    }

    #[test]
    fn unknown_embedding_fields_are_rejected() {
        let value = serde_json::json!({"provider":"ollama","url":"http://localhost:11434","model":"fixture","timeout_secs":60,"download_consent":false,"search_timeout_ms":8000,"chunk_target_tokens":1100,"implicit_fallback":true});
        assert!(serde_json::from_value::<EmbeddingSettings>(value).is_err());
    }

    #[test]
    fn failed_embedding_save_rolls_back_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDb::open(&dir.path().join("app.db")).unwrap();
        let mut config = EmbeddingSettings {
            provider: "ollama".into(),
            url: "http://localhost:11434".into(),
            model: "old".into(),
            timeout_secs: 60,
            download_consent: false,
            search_timeout_ms: 65_000,
            chunk_target_tokens: 1100,
        };
        save_embedding_settings(&db, &config).unwrap();
        db.conn().execute_batch("CREATE TRIGGER fail_config BEFORE INSERT ON settings WHEN NEW.key = 'chunk_target_tokens' BEGIN SELECT RAISE(ABORT, 'injected disk failure'); END;").unwrap();
        config.model = "new".into();
        config.timeout_secs = 90;
        config.search_timeout_ms = 95_000;
        config.download_consent = true;
        assert!(save_embedding_settings(&db, &config).is_err());
        assert_eq!(
            db.get_setting("semantic_memory_embedding_model")
                .unwrap()
                .as_deref(),
            Some("old")
        );
        assert_eq!(
            db.get_setting("semantic_memory_embedding_timeout_secs")
                .unwrap()
                .as_deref(),
            Some("60")
        );
        assert_eq!(
            db.get_setting("fastembed_download_consent")
                .unwrap()
                .as_deref(),
            Some("false")
        );
    }
    #[test]
    fn search_timeout_cannot_expire_before_embedding_timeout_budget() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDb::open(&dir.path().join("app.db")).unwrap();
        let before_provider = db
            .get_setting("semantic_memory_embedding_provider")
            .unwrap();
        let before_model = db.get_setting("semantic_memory_embedding_model").unwrap();
        let config = EmbeddingSettings {
            provider: "ollama".into(),
            url: "http://localhost:11434".into(),
            model: "fixture".into(),
            timeout_secs: 10,
            download_consent: false,
            search_timeout_ms: 8_000,
            chunk_target_tokens: 1100,
        };
        let error = save_embedding_settings(&db, &config).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("must allow embedding timeout plus retrieval overhead"),
            "unexpected error: {error}"
        );
        assert_eq!(
            db.get_setting("semantic_memory_embedding_provider")
                .unwrap(),
            before_provider,
            "invalid settings must not partially change the provider"
        );
        assert_eq!(
            db.get_setting("semantic_memory_embedding_model").unwrap(),
            before_model,
            "invalid settings must not partially change the model"
        );
    }

    #[test]
    fn invalid_numeric_enum_and_boolean_values_are_rejected() {
        for (key, value) in [
            ("chunk_target_tokens", "0"),
            ("chunk_target_tokens", "3001"),
            ("semantic_memory_embedding_timeout_secs", "1"),
            ("semantic_memory_embedding_timeout_secs", "301"),
            ("semantic_memory_search_timeout_ms", "0"),
            ("generation_temperature", "NaN"),
            ("generation_top_p", "1.1"),
            ("allow_lan_local_providers", "yes"),
            ("semantic_memory_embedding_provider", "automatic"),
            ("semantic_memory_embedding_model", " "),
        ] {
            assert!(validate_setting_value(key, value).is_err(), "{key}={value}");
        }
        assert!(validate_setting_value("chunk_target_tokens", "1100").is_ok());
    }
    #[test]
    fn embedding_apply_validates_before_writing_and_only_identity_changes_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDb::open(&dir.path().join("app.db")).unwrap();
        let mut config = EmbeddingSettings {
            provider: "ollama".into(),
            url: "http://localhost:11434".into(),
            model: "fixture".into(),
            timeout_secs: 60,
            download_consent: false,
            search_timeout_ms: 65_000,
            chunk_target_tokens: 1100,
        };
        save_embedding_settings(&db, &config).unwrap();
        config.model = "changed".into();
        config.timeout_secs = 0;
        assert!(save_embedding_settings(&db, &config).is_err());
        assert_eq!(
            db.get_setting("semantic_memory_embedding_model")
                .unwrap()
                .as_deref(),
            Some("fixture")
        );
        config.model = "fixture".into();
        config.timeout_secs = 90;
        config.search_timeout_ms = 95_000;
        assert!(!save_embedding_settings(&db, &config).unwrap());
        config.model = "changed".into();
        assert!(save_embedding_settings(&db, &config).unwrap());
        config.url = "http://192.168.1.2:11434".into();
        assert!(save_embedding_settings(&db, &config).is_err());
        assert_eq!(
            db.get_setting("semantic_memory_embedding_url")
                .unwrap()
                .as_deref(),
            Some("http://localhost:11434")
        );
        config.provider = "fastembed".into();
        // A disabled, irrelevant LAN URL must not block local-only recovery.
        assert!(save_embedding_settings(&db, &config).unwrap());

        config.url = "http://irrelevant.invalid:11434".into();
        config.model = "irrelevant-remote-model".into();
        config.timeout_secs = 120;
        assert!(
            !save_embedding_settings(&db, &config).unwrap(),
            "remote-only fields must not invalidate the fixed local Candle identity"
        );
    }

    #[test]
    fn proven_search_budget_covers_embedding_probe_and_turbo_quant_cold_start() {
        assert_eq!(minimum_semantic_search_timeout_ms(10), 15_000);
        assert_eq!(
            proven_semantic_search_timeout_ms(8_000, 10, 2_000, false),
            15_000
        );
        assert_eq!(
            proven_semantic_search_timeout_ms(8_000, 10, 23_870, true),
            60_000
        );
        assert_eq!(
            proven_semantic_search_timeout_ms(8_000, 10, 80_000, true),
            165_000
        );
        assert_eq!(
            proven_semantic_search_timeout_ms(8_000, 300, 300_000, true),
            MAX_SEMANTIC_SEARCH_TIMEOUT_MS
        );
    }
}
