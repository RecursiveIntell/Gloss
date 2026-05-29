# Timeout, Streaming, Partial Output, and Continuation Spec

## Requirements

- Increase relevant generation timeouts by 40% and write `TIMEOUT_CHANGE_RECEIPT.md` listing old/new values and files touched.
- Preserve partial output on timeout, stream error, manual cancel, and notebook epoch cancellation where safe.
- Store partial output digest and message status.
- Add continuation action that continues from captured partial answer and prior context.
- Mark continued answers with lineage to previous attempt.
- Do not silently convert partial failure into complete answer.

## Acceptance gates

- Provider start timeout fixture records `partial_timeout` or explicit no-content timeout.
- Stream idle timeout fixture preserves partial tokens.
- Continuation from partial produces a new linked answer.
- UI shows partial status and continuation action.
