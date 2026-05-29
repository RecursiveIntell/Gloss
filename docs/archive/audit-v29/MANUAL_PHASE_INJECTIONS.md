# Manual Phase-Injection Prompts

Paste the relevant guardrail before allowing Codex to continue.

## After P00

Before edits, prove with file paths and command output which chat branch currently fails or state that it is not yet proven. If unproven, implement diagnostics first only.

## After P01

Show the UI/backend path that exposes `debug_chat_provider_smoke` and `get_last_chat_attempt_trace`. If the operator still cannot get a trace, do not proceed.

## After P02

Prove source-list loading/partial/error no longer disables chat. Show the test/static gate.

## After P03

Prove chat lifecycle events route by active stream identity, not active notebook view. Show code and test.

## After P04

List every spawned-task return path and the terminal event emitted. If any return lacks terminalization, stop.

## After P05

Prove `chat:done` cannot occur before assistant persistence. Show mock DB failure test.

## After P06

Show provider URL class, selected model, model availability, and live smoke trace. If LAN support was added, prove it is opt-in and bounded.

## After P08

Fresh unzip/package replay must show no missing script refs, current-run truth agreement, and structured gate result. If not, release remains blocked.

## Before final answer

Populate the hostile-auditor handoff. Do not summarize success without changed files, commands, tests, receipts, and blockers.
