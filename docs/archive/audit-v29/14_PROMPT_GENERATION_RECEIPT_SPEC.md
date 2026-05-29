# Prompt and Generation Receipt Spec

## PromptReceiptV1

Must record:

- answer/message id;
- prompt assembly version;
- system prompt capture state: captured, redacted, not_captured, not_implemented, unknown;
- system prompt digest and optional redacted preview;
- user turn digest;
- context payload digest;
- source ids and chunk ids in context;
- citation anchor count;
- redaction state and reason;
- warnings.

## GenerationReceiptV1

Must record:

- answer/message id;
- provider/model route;
- request attempt id;
- DecodingSettingsReceiptV1 ref;
- PromptReceiptV1 ref;
- RetrievalCapabilityDecisionV1 ref;
- provider start timeout;
- stream idle timeout;
- elapsed ms;
- status: queued, retrieving_context, generating, partial_timeout, partial_error, cancelled, complete, continued;
- partial output digest when partial;
- final output digest when complete;
- token counts if provider returns them;
- provider error/degraded state.

## Privacy policy

Prompt Inspector may show redacted previews. It must never reconstruct missing prompts. For old answers, show `not_captured`.
