# Inspector Dock and UI Spec

## Required tabs

1. Notes — preserve current notes behavior.
2. Prompt — PromptReceiptV1, prompt previews, prompt digests, effective decoding settings.
3. Evidence — retrieval decision, citations, source/chunk anchors, fallback/degraded reasons.
4. Receipt — generation receipt, semantic runtime truth, command/attempt status.
5. Sources — source health, dense/FTS/projection/summary state for selected scope.

## Honesty states

Every field must render one of:

```text
captured
computed_from_captured_state
redacted
unknown
not_captured
not_implemented
unsupported_by_provider
provider_default_unknown
```

## Prohibitions

- No fake prompt reconstruction.
- No fake groundedness score.
- No semantic-memory “used” badge unless backend decision proves it.
- No TurboQuant “used” badge unless exact proof exists.
- No current settings displayed as old answer settings.

## Acceptance gates

- Inspector Dock tabs render.
- Notes still work.
- Selecting an old answer shows `not_captured` where receipts are missing.
- Selecting a new answer shows prompt/generation/decoding/retrieval receipts.
