# Settings control contract

Source audit and implementation: 2026-09-05, production hardening pass based on `8c7cf7d1249f50baeae1b08980d64d97d89a6ffc`.

Settings correctness is partially verified. The real settings contract, SQLite persistence, feature policy and frontend store/render tests pass. A complete Tauri desktop session, every external provider and restart/recovery against a real user installation have not been certified here.

## Control inventory

| Visible control or setting | Canonical owner and persistence | Apply and validation | Effect and proof boundary |
|---|---|---|---|
| Ollama, llama.cpp, OpenAI and Anthropic server URLs | `providers` table through `update_provider`; displayed settings URL keys are projections | Explicit Save or save-before-Test; provider URL policy checks loopback/LAN/cloud authority | Provider config is rebuilt. Settings UI preserves an unrelated provider's pending draft. External provider connectivity remains a live-host gate. |
| OpenAI and Anthropic API key, Show/Hide, Clear | Device secret store; SQLite retains no plaintext key | Explicit Save/Clear; blank input preserves an existing key unless Clear is used | Show/Hide affects the current input only. Save/Clear pending controls prevent conflicting edits. Secret-store disk failure paths require supported-host verification. |
| Allow LAN local providers | AppDb `allow_lan_local_providers` | Strict boolean, serialized save, acknowledgement before displayed change | Current provider and embedding policy reads this authority. Background jobs revalidate it at dispatch. Revocation cannot authorize a new send to the old LAN endpoint. |
| Allow custom cloud endpoints | AppDb `allow_custom_cloud_endpoints` | Strict boolean; HTTPS/provider policy still required | Enables supported non-default cloud origins explicitly; does not allow arbitrary embedding endpoints. |
| Chat model/provider dropdown and model list | AppDb `default_provider` plus `default_model` as one transaction | Exact provider/model pair, available and nonstale, enabled provider | No bare-name selection across providers. Discovery refresh cannot erase configured intent. |
| Chat temperature | AppDb `generation_temperature` through `update_setting` and the serialized settings store | Explicit Apply, finite 0–2, default 0.7; saved state changes only after acknowledgement | Ollama, OpenAI and llama.cpp adapters forward the value. Anthropic control is disabled and its receipts identify provider-managed defaults. Unsaved drafts survive settings refreshes and save failures while the dialog remains open. Live cloud-model compatibility is not certified. |
| Refresh Models and provider Test | Current provider registry and provider transport | Explicit action with reachable/error result | Connectivity tests do not certify model quality, embedding readiness or full chat. |
| Embedding backend | AppDb `semantic_memory_embedding_provider` | Complete draft applied by `update_embedding_settings`; UI offers Ollama or built-in CPU | Exactly selected backend; no automatic Ollama-to-CPU fallback promise. Native service checks current configured identity. |
| Embedding URL and model | Same atomic embedding configuration | Validated only when Ollama is selected; URL authority and nonempty model required | Identity changes mark existing indexes stale. Empty notebooks are not marked stale. Built-in recovery remains possible after LAN authority is revoked. |
| Embedding timeout | AppDb `semantic_memory_embedding_timeout_secs` | Integer 2–300 seconds, complete Apply | Applies to Ollama operations. A timeout-only change does not invalidate durable vector identity. |
| Automatically download embedding model | AppDb `fastembed_download_consent` | Boolean in complete Apply; explicit false survives restart | Built-in model download requires consent. Existing cached model may be used without downloading. Native startup seeds missing consent as false. |
| Search timeout | AppDb `semantic_memory_search_timeout_ms` | Integer 100–300000 milliseconds in complete Apply | Used by semantic retrieval timeout. Does not change embedding identity. |
| Chunk target tokens | AppDb `chunk_target_tokens` | Integer 100–3000 in complete Apply | Applies to future imports/reimports. Existing canonical chunks are not silently rewritten. |
| Apply embedding and ingestion settings | `settings_contract.rs` and AppDb atomic transaction | All fields validate before any write; failed late write rolls back every field | No debounce or per-keystroke persistence. UI shows unsaved state and disables draft editing during Apply. Settings acknowledgement and index invalidation warnings are separate. |
| Gloss local / Semantic memory (safe) / Strict retrieval profile | `commands/settings.rs` profile transaction | Compiled capability check, notebook projection proof and strict TurboQuant proof when compiled | Atomically controls backend, fallback, auto-projection and strict policy. Legacy feature bits do not determine build capability. |
| Fallback, auto-project, fresh-artifact checkboxes | Profile-owned AppDb settings, displayed read-only | Changed by supported profile selection | Display the selected profile, not independent switches that can create inconsistent partial configurations. |
| Use proveKV pool candidates | AppDb `semantic_memory_provekv_pool_candidates_enabled` | Strict boolean; disabled when TurboQuant is absent | Runtime candidate policy changes, while exact f32 rerank remains required. Runtime proof belongs to the TurboQuant canary. |
| Projection backfill, artifact rebuild, retrieval probe, embedding diagnostics, Refresh evidence | Existing notebook command owners and receipts | Explicit action and notebook/build prerequisites | Diagnostics report observations. They do not promote unavailable runtime work to successful proof. Full native execution remains a desktop gate. |
| Summary model | AppDb `summary_model`; Ollama is the supported background provider | Available, nonstale Ollama models only; old incompatible selection remains visible | Same-as-chat is unavailable for a non-Ollama chat provider. Enqueue and dispatch use canonical provider/model/network policy. |
| Vision model | AppDb `vision_model`; Ollama background provider | Available vision-capable Ollama model; incompatible chat default visibly requires repair | Same identity and dispatch checks as summaries. Media tooling must separately be available. |
| Manual / automatic summaries, generate missing | Dedicated summary queue commands plus persisted `summary_mode`; explicit one-shot intent lives in each summary payload | Automatic admission requires Auto. Generate missing in Manual persists `explicit_requested=true` without changing mode. Pause cancels pending and processing summaries only. | Manual mode no longer stalls or cancels media, audio or indexing. Dispatch rechecks mode and cancellation. Legacy payloads default to automatic. Generic `update_setting(summary_mode)` is rejected. |
| Database doctor Check / Repair | Existing doctor command/receipt | Explicit operation, repair separate from check | Not an embedding retry or an implicit settings reset. Real user-data repair requires its own receipt. |
| Local RAG, source scope, background, vision, media, diagnostics capability badges | Build/implemented capability descriptors | Read-only; ineffective mutable APIs rejected | Old stored values are not allowed to make an always-available runtime capability appear disabled. |
| Semantic-memory and TurboQuant capability rows | Actual build feature availability | Read-only, consistent `enabled`, `active`, `available` fields | Existing always-on build policy is preserved. There is no ineffective runtime enable/disable checkbox. |
| Experimental master, Advanced Retrieval, Index Replay, Package Release controls | Legacy persisted keys remain readable; controls unavailable | No mutable UI switch or accepted mutation pretending to implement these controls | Not implemented is shown explicitly. An obsolete master switch cannot change the selected memory backend. |
| Studio widget preferences | Existing feature flags consumed by Studio | Feature update API; defaults seed missing keys only | Explicit false survives default initialization. No reseed overwrites a user opt-out. |
| Theme | `uiStore`, device localStorage `gloss:theme`; command palette or Ctrl/Cmd+Shift+T | Dark/light immediately apply to document theme; storage errors preserve usable in-memory state | Generic AppDb theme writes are rejected. There is one active UI preference owner. |
| Source/Inspector widths and collapse state | `PanelLayout` localStorage `gloss:layout:*` | Widths finite and clamped, immediate UI change | Separate device preference projection. Responsive browser behavior belongs to the UI verification lane. |
| Chat style, custom goal, response length | `chatStore` per-session request state | Passed with each send; visible prompt receipt captures the actual assembled prompt | These are session controls, not durable application settings. Context limit comes from provider/model information and backend request sizing. |
| System prompt visibility | Captured prompt receipt, `PromptPanel` | Read-only inspect/copy actual prompt | Does not synthesize a historical prompt from current settings. |
| Import/export/reset | No general settings import/export/reset controls exist | Notebook import/export is owned by notebook commands and is not a settings backup | No invented universal reset or settings portability claim. |

## Confirmed defects repaired

1. Partial embedding identity typing persisted and invalidated every notebook. Explicit atomic Apply now validates the complete draft and fences publication through existing-index invalidation.
2. An older optimistic-save failure could overwrite a newer setting. Generic and embedding configuration writes now serialize and display acknowledged state only.
3. Any settings reload discarded another provider's unfinished URL/key draft. Provider buffers synchronize only when not dirty, and Save/Clear edits are guarded while pending.
4. Summary/vision menus advertised providers the job runner rejected and used ambiguous model names. Menus and runtime now bind the supported Ollama identity, retain unavailable configured intent, and validate current dispatch authority.
5. Build-only memory capabilities were displayed as mutable legacy preview flags. UI/status/profile handling now reflects actual compiled availability and supported runtime profile controls.
6. Invalid enums, booleans, nonfinite numbers and out-of-range settings could persist successfully but fail or silently clamp later. The canonical settings contract rejects them before persistence.
7. Startup reset explicit model-download and widget opt-outs. Missing values alone receive defaults.
8. Revoking LAN access made switching to built-in CPU fail on an inactive, disabled URL field. Only the selected network backend's endpoint is validated.
9. Manual summaries paused the shared queue and blocked media/index ingestion, while Generate silently enabled Auto. Summary-only admission/cancellation now preserves ingestion and the user's mode; explicit one-shot intent persists in each requested job.

## Validation and remaining gates

The shared queue currently defers every job until a notebook is selected, chat's 15-second grace period expires and synchronous imports release their gate. The queue core has no filtered-claim API. Job resource-policy flags describe ownership/cancellation requirements, not independently scheduled workers. Manual mode itself no longer blocks ingestion. Fully independent ingestion dispatch remains a bounded scheduling limitation, not a completed claim.

- `npm run test:unit -- src/components/settings/SettingsDialog/settings.render.test.ts src/stores/__tests__/settingsStore.test.ts`: 7 passed.
- Native harness `settings_contract::tests`: 5 passed, including late SQLite-write rollback, unknown fields, numerical validation, empty-notebook invalidation and local recovery after LAN revocation.
- Native harness `features::tests`: 7 passed with actual settings source, without semantic runtime features. Harness feature names are used only to exercise capability policy, not to claim the semantic runtime is compiled.
- The same feature-policy tests with the harness's `semantic-memory-turbo-quant` capability flags: 8 passed. This proves conditional settings policy only, not the semantic adapter or TurboQuant runtime.
- `npm run build`: passed on the integrated working tree after the settings changes. Bundle size is a warning, not a measured startup result.
- Full desktop interaction, real key storage, all external provider/server combinations and end-to-end native settings/restart tests remain unverified here. See the production receipt and supported-host desktop gates.
- Five real queue-core fixtures pass for legacy intent, manual automatic-summary rejection, explicit/media/index preservation, summary-only cancellation including claimed work, dispatch-time mode change and per-source deduplication. A full Tauri source test additionally covers the actual Generate command's admission path; it has not run in this environment.

## Rollback

No schema migration is introduced. Revert the settings hardening changes as one reviewed set, including IPC registration and wrapper, store/UI, settings contract and feature policy. Do not restore stale indexes as ready or delete canonical notebook chunks to disguise a failed rebuild. Rebuild derived indexes under the chosen embedding identity. Retain user preferences, secrets and notebook data.
