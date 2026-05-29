# Generation and Decoding Settings Spec

## Observed current source findings

- `ChatRequest` currently contains `model`, `system_prompt`, `messages`, `max_tokens`, `temperature`, `stream`, and `num_ctx`.
- Main chat streaming path hardcodes `temperature: 0.7` and `max_tokens: 2048`.
- Provider-only smoke uses `temperature: 0.0`.
- Summarization/vision paths have hardcoded temperatures and `num_ctx` values.
- Ollama provider maps `temperature`/`num_ctx` but other sampling fields are not first-class.
- Frontend `types.ts` includes optional `temperature`, but common settings UI does not expose full generation policy.

## Required provider capability matrix

| provider | field | supported | request_path | setting_key | default_policy | receipt_field | ui_visible | notes |
|---|---|---:|---|---|---|---|---:|---|
| ollama | temperature | yes | `options.temperature` | `provider.ollama.temperature` | user/default/model override | `effective.temperature` | yes | Docs show options including temperature. |
| ollama | top_k | verify-current | `options.top_k` | `provider.ollama.top_k` | unsupported until verified | `effective.top_k` | advanced | Verify current Ollama Modelfile/API docs during implementation. |
| ollama | top_p | verify-current | `options.top_p` | `provider.ollama.top_p` | unsupported until verified | `effective.top_p` | advanced | Verify current docs. |
| ollama | min_p | verify-current | `options.min_p` | `provider.ollama.min_p` | unsupported until verified | `effective.min_p` | advanced | Verify current docs. |
| ollama | typical_p | verify-current | `options.typical_p` | `provider.ollama.typical_p` | unsupported until verified | `effective.typical_p` | advanced | Verify current docs. |
| ollama | num_ctx | yes | `options.num_ctx` | `provider.ollama.num_ctx` | dynamic/user override | `effective.num_ctx` | basic | Current code computes dynamic num_ctx. |
| ollama | num_predict | verify-current | `options.num_predict` | `provider.ollama.num_predict` | user/default | `effective.num_predict` | basic | Equivalent to max output tokens for Ollama. |
| ollama | keep_alive | yes | request `keep_alive` | `provider.ollama.keep_alive` | provider default unless set | `effective.keep_alive` | advanced | Docs describe keep_alive. |
| openai | temperature | yes for supported models | request `temperature` | `provider.openai.temperature` | provider default unknown unless set | `effective.temperature` | yes | Recheck Responses API/model restrictions. |
| openai | top_p | yes for supported models | request `top_p` | `provider.openai.top_p` | provider default unknown unless set | `effective.top_p` | advanced | Recheck current model support. |
| openai | top_k/min_p/typical_p | no/unknown | none | none | unsupported_by_provider | unsupported list | no | Do not send. |
| anthropic | temperature/top_p/top_k | verify-current | messages body fields if supported | `provider.anthropic.*` | provider default unknown | effective/unsupported | provider-aware | Recheck official docs during implementation. |
| llamacpp | temperature/top_p/top_k/etc | endpoint-specific | request body | `provider.llamacpp.*` | unknown until probe | effective/opaque | advanced | Capability probe required. |

## Setting schema

Prefer provider-scoped keys:

```text
provider.<provider>.temperature
provider.<provider>.top_k
provider.<provider>.top_p
provider.<provider>.min_p
provider.<provider>.typical_p
provider.<provider>.repeat_last_n
provider.<provider>.repeat_penalty
provider.<provider>.presence_penalty
provider.<provider>.frequency_penalty
provider.<provider>.seed
provider.<provider>.num_predict
provider.<provider>.num_ctx
provider.<provider>.stop_sequences
provider.<provider>.keep_alive
provider.<provider>.stream
```

Also keep global defaults for initial values:

```text
generation.default_temperature
generation.default_num_predict
generation.default_num_ctx
generation.default_stream
```

## Effective settings resolution

Order:

1. provider hard capability support;
2. app default;
3. provider default if explicit and known;
4. model override;
5. prompt-mode override;
6. user settings;
7. per-request override;
8. final effective value.

Every receipt must record the source for every field:

```text
user_setting
model_override
prompt_mode_override
app_default
provider_default_known
provider_default_unknown
hardcoded_legacy
unsupported_by_provider
opaque
not_sent
```

## UI

Add Settings → Generation / Model Runtime:

- Basic: temperature, max output tokens/num_predict, context window/num_ctx, streaming on/off.
- Advanced: top_k, top_p, min_p, typical_p, repeat controls, penalties, seed, stop sequences, keep_alive.
- Provider capability badges: supported, unsupported, unknown, opaque.
- Reset to app default and reset to provider default.
- Per-answer Inspector Dock displays effective settings used, not current settings.

## Tests

- Hardcoded main chat temperature removed.
- Effective temperature appears in DecodingSettingsReceiptV1.
- Ollama top_k/top_p are sent only after verified support and user setting.
- Unsupported fields are not sent.
- Setting change affects next answer, not old receipts.
- Old answers display `not_captured` rather than reconstructed settings.
