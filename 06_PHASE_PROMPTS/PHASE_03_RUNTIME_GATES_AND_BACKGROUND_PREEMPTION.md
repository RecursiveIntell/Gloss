# Phase 03 — Runtime Gates and Background Preemption

Tasks:
1. Audit GPU/LLM gate owners and background summary jobs.
2. Foreground chat must preempt/cancel/degrade background LLM work.
3. Add gate-owner/status receipts.
4. Add UI phase state for waiting/provider-starting/streaming/stale.

Acceptance:
- blocked background summary cannot make chat appear infinite;
- chat starts or reports clear bounded status within SLO;
- all gate waits are receipted.
