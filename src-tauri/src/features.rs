use crate::db::app_db::AppDb;
use crate::error::GlossError;
use serde::Serialize;

pub const EXPERIMENTAL_FEATURES_ENABLED: &str = "experimental_features_enabled";
pub const FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED: &str = "feature_semantic_memory_preview_enabled";
pub const FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED: &str =
    "feature_semantic_memory_turbo_quant_enabled";
pub const FEATURE_CHAT_DIAGNOSTICS_ENABLED: &str = "feature_chat_diagnostics_enabled";
pub const FEATURE_PROVIDER_SMOKE_TOOLS_ENABLED: &str = "feature_provider_smoke_tools_enabled";
pub const FEATURE_ADVANCED_RETRIEVAL_CONTROLS_ENABLED: &str =
    "feature_advanced_retrieval_controls_enabled";
pub const FEATURE_INDEX_REPLAY_TOOLS_ENABLED: &str = "feature_index_replay_tools_enabled";
pub const FEATURE_PACKAGE_RELEASE_PANEL_ENABLED: &str = "feature_package_release_panel_enabled";
pub const FEATURE_VISION_JOBS_ENABLED: &str = "feature_vision_jobs_enabled";
pub const FEATURE_VIDEO_IMPORT_ENABLED: &str = "feature_video_import_enabled";
pub const FEATURE_FLASHCARD_WIDGET_ENABLED: &str = "feature_flashcard_widget_enabled";
pub const FEATURE_QUIZ_WIDGET_ENABLED: &str = "feature_quiz_widget_enabled";
pub const FEATURE_MIND_MAP_WIDGET_ENABLED: &str = "feature_mind_map_widget_enabled";
pub const FEATURE_BACKGROUND_SUMMARIES_ENABLED: &str = "feature_background_summaries_enabled";
pub const FEATURE_EXTERNAL_TOOLS_ENABLED: &str = "feature_external_tools_enabled";
pub const FEATURE_LOCAL_RAG_ENABLED: &str = "feature_local_rag_enabled";
pub const FEATURE_SOURCE_SCOPE_ENABLED: &str = "feature_source_scope_enabled";

pub const MEMORY_BACKEND_GLOSS_LOCAL: &str = "gloss-local";
pub const MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW: &str = "semantic-memory-preview";
pub const SEMANTIC_MEMORY_AUTO_PROJECT: &str = "semantic_memory_auto_project";
pub const SEMANTIC_MEMORY_STRICT_TESTING: &str = "semantic_memory_strict_testing";
pub const SEMANTIC_MEMORY_TURBO_QUANT_REQUIRE_FRESH_ARTIFACTS: &str =
    "semantic_memory_turbo_quant_require_fresh_artifacts";
pub const SEMANTIC_MEMORY_PROVEKV_POOL_CANDIDATES_ENABLED: &str =
    "semantic_memory_provekv_pool_candidates_enabled";
pub const FASTEMBED_DOWNLOAD_CONSENT: &str = "fastembed_download_consent";
// Settings profiles expose use_gloss_local_profile and enable_semantic_memory_profile flows.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildFeature {
    SemanticMemoryBackend,
    SemanticMemoryTurboQuant,
}

#[derive(Debug, Clone, Copy)]
pub struct FeatureDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub section: &'static str,
    pub description: &'static str,
    pub default_enabled: bool,
    pub stable: bool,
    pub requires_experimental: bool,
    pub build_feature: Option<BuildFeature>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FeatureFlagStatus {
    pub id: String,
    pub label: String,
    pub section: String,
    pub description: String,
    pub enabled: bool,
    pub active: bool,
    pub available: bool,
    pub stable: bool,
    pub default_enabled: bool,
    pub requires_experimental: bool,
    pub unavailable_reason: Option<String>,
}

pub fn feature_definitions() -> &'static [FeatureDefinition] {
    &[
        FeatureDefinition {
            id: EXPERIMENTAL_FEATURES_ENABLED,
            label: "Experimental Features",
            section: "Experimental Features",
            description: "Master switch for preview and validation-only surfaces.",
            default_enabled: false,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_LOCAL_RAG_ENABLED,
            label: "Local RAG",
            section: "Memory & Retrieval",
            description: "Gloss local retrieval and citations.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_SOURCE_SCOPE_ENABLED,
            label: "Source Scope",
            section: "Sources & Ingestion",
            description: "Explicit source selection for chat context.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_BACKGROUND_SUMMARIES_ENABLED,
            label: "Background Summaries",
            section: "Summaries",
            description: "Queue-driven local source summaries.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_VISION_JOBS_ENABLED,
            label: "Vision Jobs",
            section: "Vision & Media",
            description: "Image description jobs through configured local models.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_VIDEO_IMPORT_ENABLED,
            label: "Video Import",
            section: "Vision & Media",
            description: "Video frame extraction through external media tools.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_EXTERNAL_TOOLS_ENABLED,
            label: "External Tools",
            section: "External Tools",
            description: "ffmpeg and ffprobe availability checks.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_CHAT_DIAGNOSTICS_ENABLED,
            label: "Chat Diagnostics",
            section: "Diagnostics",
            description: "Chat attempt trace and visible runtime diagnostics.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_PROVIDER_SMOKE_TOOLS_ENABLED,
            label: "Provider Smoke Tools",
            section: "Diagnostics",
            description: "Provider-only chat smoke command for diagnostics.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED,
            label: "semantic-memory Preview",
            section: "Experimental Features",
            description: "Optional preview backend; Gloss local remains default.",
            default_enabled: false,
            stable: false,
            requires_experimental: true,
            build_feature: Some(BuildFeature::SemanticMemoryBackend),
        },
        FeatureDefinition {
            id: FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED,
            label: "TurboQuant Candidates",
            section: "Experimental Features",
            description: "Candidate acceleration only; exact rerank remains required.",
            default_enabled: false,
            stable: false,
            requires_experimental: true,
            build_feature: Some(BuildFeature::SemanticMemoryTurboQuant),
        },
        FeatureDefinition {
            id: FEATURE_ADVANCED_RETRIEVAL_CONTROLS_ENABLED,
            label: "Advanced Retrieval Controls",
            section: "Experimental Features",
            description: "Preview retrieval controls outside the release default path.",
            default_enabled: false,
            stable: false,
            requires_experimental: true,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_INDEX_REPLAY_TOOLS_ENABLED,
            label: "Index Replay Tools",
            section: "Release / Validation",
            description: "Validation-only index replay controls.",
            default_enabled: false,
            stable: false,
            requires_experimental: true,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_PACKAGE_RELEASE_PANEL_ENABLED,
            label: "Package Release Panel",
            section: "Release / Validation",
            description: "Validation-only release evidence controls.",
            default_enabled: false,
            stable: false,
            requires_experimental: true,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_FLASHCARD_WIDGET_ENABLED,
            label: "Flashcard Widget",
            section: "Studio",
            description: "Interactive card-flip study widget for studio flashcard outputs.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_QUIZ_WIDGET_ENABLED,
            label: "Quiz Widget",
            section: "Studio",
            description: "Interactive multiple-choice quiz with scoring and explanations.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
        FeatureDefinition {
            id: FEATURE_MIND_MAP_WIDGET_ENABLED,
            label: "Mind Map Graph",
            section: "Studio",
            description: "Interactive d3-force mind map visualization for studio graph outputs.",
            default_enabled: true,
            stable: true,
            requires_experimental: false,
            build_feature: None,
        },
    ]
}

/// Marker for the one-time re-seed of the Studio widget flags: earlier builds
/// seeded them to false while the widgets could not render real data, so a
/// stored "false" predating this marker reflects the old default, not a user
/// choice.
const STUDIO_WIDGET_FLAGS_RESEEDED: &str = "studio_widget_flags_reseeded_v2";

pub fn ensure_default_feature_settings(app_db: &AppDb) -> Result<(), GlossError> {
    for definition in feature_definitions() {
        if app_db.get_setting(definition.id)?.is_none() {
            app_db.set_setting(definition.id, bool_to_setting(definition.default_enabled))?;
        }
    }
    if app_db.get_setting(STUDIO_WIDGET_FLAGS_RESEEDED)?.is_none() {
        for id in [
            FEATURE_FLASHCARD_WIDGET_ENABLED,
            FEATURE_QUIZ_WIDGET_ENABLED,
            FEATURE_MIND_MAP_WIDGET_ENABLED,
        ] {
            app_db.set_setting(id, bool_to_setting(true))?;
        }
        app_db.set_setting(STUDIO_WIDGET_FLAGS_RESEEDED, "true")?;
    }
    Ok(())
}

pub fn feature_flag_statuses(app_db: &AppDb) -> Result<Vec<FeatureFlagStatus>, GlossError> {
    ensure_default_feature_settings(app_db)?;
    let experimental_master = setting_bool(app_db, EXPERIMENTAL_FEATURES_ENABLED, false)?;
    feature_definitions()
        .iter()
        .map(|definition| {
            let enabled = setting_bool(app_db, definition.id, definition.default_enabled)?;
            let build_available = build_feature_available(definition.build_feature);
            let dependency_available = feature_dependencies_available(app_db, definition)?;
            let available = build_available
                && dependency_available
                && (!definition.requires_experimental
                    || experimental_master
                    || definition.id == EXPERIMENTAL_FEATURES_ENABLED);
            let active = feature_active(app_db, definition, enabled, available)?;
            Ok(FeatureFlagStatus {
                id: definition.id.to_string(),
                label: definition.label.to_string(),
                section: definition.section.to_string(),
                description: definition.description.to_string(),
                enabled,
                active,
                available,
                stable: definition.stable,
                default_enabled: definition.default_enabled,
                requires_experimental: definition.requires_experimental,
                unavailable_reason: unavailable_reason(app_db, definition, experimental_master)?,
            })
        })
        .collect()
}

pub fn update_feature_flag(app_db: &AppDb, id: &str, enabled: bool) -> Result<(), GlossError> {
    ensure_default_feature_settings(app_db)?;
    let definition = find_feature_definition(id)
        .ok_or_else(|| GlossError::Config(format!("Unknown feature flag '{id}'")))?;
    if enabled && !build_feature_available(definition.build_feature) {
        return Err(GlossError::Config(format!(
            "Feature '{}' is not available in this build",
            definition.id
        )));
    }
    if enabled && definition.id == FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED {
        require_semantic_memory_preview_enabled(app_db)?;
    }
    app_db.set_setting(definition.id, bool_to_setting(enabled))?;
    if definition.id == EXPERIMENTAL_FEATURES_ENABLED && !enabled {
        reset_experimental_runtime_settings(app_db)?;
    }
    Ok(())
}

pub fn validate_setting_update(app_db: &AppDb, key: &str, value: &str) -> Result<(), GlossError> {
    if key == "memory_backend" && value == MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW {
        require_semantic_memory_preview_enabled(app_db)?;
    }
    if key == FASTEMBED_DOWNLOAD_CONSENT && !matches!(value, "true" | "false") {
        return Err(GlossError::Config(
            "fastembed_download_consent must be true or false".into(),
        ));
    }
    Ok(())
}

pub fn apply_setting_update_side_effects(
    app_db: &AppDb,
    key: &str,
    value: &str,
) -> Result<(), GlossError> {
    if key == EXPERIMENTAL_FEATURES_ENABLED && !setting_value_is_enabled(value) {
        reset_experimental_runtime_settings(app_db)?;
    }
    // C4 — When the embedding model identity changes, the cached query
    // embeddings (in state.query_embed_cache) and the HNSW index (in
    // state.hnsw_indices) are stale. The query cache is auto-flushed by
    // AppState::ensure_embedder on next call; the HNSW index is checked by
    // dimension in ensure_hnsw_index. Here we just log so the user
    // notices that changing the model triggers a re-index.
    if key == "semantic_memory_embedding_model" {
        let prior = app_db
            .get_setting("semantic_memory_embedding_model")
            .ok()
            .flatten();
        if prior.as_deref() != Some(value) {
            tracing::warn!(
                prior = ?prior,
                new = %value,
                "embedding model changed; HNSW index will be re-created on next chat"
            );
        }
    }
    Ok(())
}

pub fn require_semantic_memory_preview_enabled(_app_db: &AppDb) -> Result<(), GlossError> {
    Ok(())
}

pub fn semantic_memory_preview_active(_app_db: &AppDb) -> Result<bool, GlossError> {
    Ok(true)
}

pub fn turbo_quant_active(_app_db: &AppDb) -> Result<bool, GlossError> {
    Ok(cfg!(feature = "semantic-memory-turbo-quant"))
}

fn reset_experimental_runtime_settings(app_db: &AppDb) -> Result<(), GlossError> {
    if app_db
        .get_setting("memory_backend")?
        .as_deref()
        .is_some_and(|value| value == MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW)
    {
        app_db.set_setting("memory_backend", MEMORY_BACKEND_GLOSS_LOCAL)?;
    }
    Ok(())
}

fn find_feature_definition(id: &str) -> Option<&'static FeatureDefinition> {
    feature_definitions()
        .iter()
        .find(|definition| definition.id == id)
}

fn build_feature_available(feature: Option<BuildFeature>) -> bool {
    match feature {
        None => true,
        Some(BuildFeature::SemanticMemoryBackend) => cfg!(feature = "semantic-memory-backend"),
        Some(BuildFeature::SemanticMemoryTurboQuant) => {
            cfg!(feature = "semantic-memory-turbo-quant")
        }
    }
}

fn feature_dependencies_available(
    app_db: &AppDb,
    definition: &FeatureDefinition,
) -> Result<bool, GlossError> {
    if definition.id == FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED {
        return semantic_memory_preview_active(app_db);
    }
    Ok(true)
}

fn feature_active(
    app_db: &AppDb,
    definition: &FeatureDefinition,
    enabled: bool,
    available: bool,
) -> Result<bool, GlossError> {
    if definition.id == FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED {
        return turbo_quant_active(app_db);
    }
    Ok(enabled && available)
}

fn unavailable_reason(
    app_db: &AppDb,
    definition: &FeatureDefinition,
    experimental_master: bool,
) -> Result<Option<String>, GlossError> {
    if !build_feature_available(definition.build_feature) {
        return Ok(Some("Not available in this build".to_string()));
    }
    if definition.id == FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED
        && !semantic_memory_preview_active(app_db)?
    {
        return Ok(Some(
            "Enable semantic-memory Preview before enabling TurboQuant Candidates".to_string(),
        ));
    }
    if definition.requires_experimental && !experimental_master {
        return Ok(Some("Experimental Features is off".to_string()));
    }
    Ok(None)
}

fn setting_bool(app_db: &AppDb, key: &str, default: bool) -> Result<bool, GlossError> {
    Ok(app_db
        .get_setting(key)?
        .as_deref()
        .map(setting_value_is_enabled)
        .unwrap_or(default))
}

fn setting_value_is_enabled(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "enabled"
    )
}

fn bool_to_setting(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db() -> AppDb {
        let dir = tempdir().unwrap();
        let path = dir.path().join("feature_test.db");
        AppDb::open(&path).unwrap()
    }

    #[test]
    fn feature_defaults_are_inserted() {
        let db = test_db();
        ensure_default_feature_settings(&db).unwrap();
        assert_eq!(
            db.get_setting(EXPERIMENTAL_FEATURES_ENABLED).unwrap(),
            Some("false".to_string())
        );
        assert_eq!(
            db.get_setting(FEATURE_LOCAL_RAG_ENABLED).unwrap(),
            Some("true".to_string())
        );
        assert_eq!(
            db.get_setting(FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED)
                .unwrap(),
            Some("false".to_string())
        );
    }

    #[test]
    fn unknown_feature_flag_is_rejected() {
        let db = test_db();
        let err = update_feature_flag(&db, "feature_missing_enabled", true).unwrap_err();
        assert!(err.to_string().contains("Unknown feature flag"));
    }

    #[test]
    fn semantic_memory_backend_always_available() {
        let db = test_db();
        ensure_default_feature_settings(&db).unwrap();
        // Semantic-memory is now always active — no gate to check.
        let is_active = semantic_memory_preview_active(&db).unwrap();
        assert!(is_active);
    }

    #[test]
    fn master_off_resets_preview_memory_backend() {
        let db = test_db();
        ensure_default_feature_settings(&db).unwrap();
        db.set_setting("memory_backend", MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW)
            .unwrap();
        update_feature_flag(&db, EXPERIMENTAL_FEATURES_ENABLED, false).unwrap();
        assert_eq!(
            db.get_setting("memory_backend").unwrap(),
            Some(MEMORY_BACKEND_GLOSS_LOCAL.to_string())
        );
    }

    #[cfg(feature = "semantic-memory-turbo-quant")]
    #[test]
    fn turbo_quant_active_when_semantic_memory_is_always_on() {
        let db = test_db();
        ensure_default_feature_settings(&db).unwrap();
        // turbo_quant_active() now only checks the build feature — always true.
        assert!(turbo_quant_active(&db).unwrap());
    }

    #[cfg(feature = "semantic-memory-turbo-quant")]
    #[test]
    fn turbo_quant_update_allowed_with_always_on_semantic_memory() {
        let db = test_db();
        ensure_default_feature_settings(&db).unwrap();
        // Semantic-memory is always-on now — enabling turbo_quant should work.
        let result = update_feature_flag(&db, FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED, true);
        assert!(result.is_ok());
    }

    #[cfg(not(feature = "semantic-memory-backend"))]
    #[test]
    fn semantic_memory_preview_flag_rejects_unavailable_build() {
        let db = test_db();
        ensure_default_feature_settings(&db).unwrap();
        update_feature_flag(&db, EXPERIMENTAL_FEATURES_ENABLED, true).unwrap();
        let err =
            update_feature_flag(&db, FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED, true).unwrap_err();
        assert!(err.to_string().contains("not available in this build"));
    }
}
