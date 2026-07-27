# Gloss Studio Provider Deduplication Plan

**Goal**: Unify the LLM provider resolution stack between chat and studio paths, eliminating duplicated configuration with diverged defaults.

**Architecture**: Extract a shared `resolve_llm_config()` function in `providers/mod.rs` that both chat and studio call. Studio keeps its own timeout constants and non-streaming choice — those are intentional.

**Status**: Council graph `gloss-studio-refactor-council` running async for design review.

---

## Finding Summary (F-06)

| Axis | Chat | Studio | Problem |
|------|------|--------|---------|
| Provider resolution | `model_registry.get_provider_config_for_model()` | `provider_config_from_db()` directly | Bypasses registry |
| Model fallback | From registry/model list | Hardcoded `"qwen3.5:4b"` | Silent wrong-model risk |
| Temperature | From `generation_temperature` setting (default 0.7) | Hardcoded 0.3 / 0.2 | Ignores user preference |
| `num_ctx` | Dynamic from model's context window | Hardcoded 16384 | May overflow or underuse |
| `max_tokens` | Dynamically computed | Hardcoded 4096 | Not configurable |
| Timeouts | 180s / 168s / 84s | 60s / 60s / 30s | Intentional — keep separate |

---

## Files to Modify

1. **`src-tauri/src/providers/mod.rs`** — Add shared `resolve_llm_config()` function
2. **`src-tauri/src/commands/studio.rs`** — Replace `run_studio_llm()` provider block (~line 702-720)
3. **`src-tauri/src/commands/chat/mod.rs`** — Optional: refactor to use same shared function (~line 776-799)

---

## Proposed Shared Function

```rust
// In providers/mod.rs
pub struct ResolvedLlmConfig {
    pub config: ProviderConfig,
    pub model: String,
    pub model_context_window: Option<i32>,
}

/// Resolve provider config and model for LLM calls.
/// Uses the model registry when available, falls back to direct DB lookup.
/// model_override: if Some, use this model; if None, use default_model from settings.
pub fn resolve_llm_config(
    app_db: &AppDb,
    secret_store: &SecretStore,
    model_registry: Option<&ModelRegistry>,
    model_override: Option<&str>,
) -> Result<ResolvedLlmConfig, GlossError> {
    // ...
}
```

## What to Keep Separate

- **Studio timeouts**: 60s/60s/30s are intentionally shorter — Studio is batch generation, not interactive chat. Keep `STUDIO_*_TIMEOUT` constants.
- **Studio non-streaming**: `stream: false` is correct for structured JSON generation. Keep.
- **Studio max_tokens**: Should be configurable but defaults can differ from chat.

## Tests to Gate

- Existing 189 Rust tests must keep passing
- Add: test that `resolve_llm_config()` returns same config as chat's current path
- Add: test that studio generation still works with the unified path
- Run: `cargo test --features semantic-memory-turbo-quant`
