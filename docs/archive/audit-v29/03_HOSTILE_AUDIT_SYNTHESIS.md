# Hostile Audit Synthesis

## S0 blockers

1. **Release receipts remain negative.** The app has not proven live dense indexing, live semantic-memory projection, live semantic-memory answer path, live TurboQuant contribution, or live desktop smoke.
2. **semantic-memory evidence is fragmented.** Settings can be enabled while answer evidence still falls back. A backend-authored runtime truth receipt is missing.
3. **`semantic_memory_feature_disabled` is overbroad.** Forced local fallback can append feature-disabled even when the actual blocker is projection/link/search failure.
4. **`RetrievalCapabilityDecisionV1` is not canonical.** It exists but does not govern all answer truth surfaces.
5. **Temperature and decoding settings are hardcoded/under-modeled.** Answer generation uses `temperature: 0.7`; provider options like `top_k`/`top_p` are not first-class settings/receipts.
6. **Prompt/generation receipts are incomplete.** Prompt construction, effective decoding settings, source context, timeout state, and final answer status are not captured in a complete per-answer receipt.
7. **Timeout/partial/continuation flow is not release-proof.** Long local generation can fail without a clear partial-state and continuation receipt path.
8. **Package/current-run truth drift persists.** The codex-archive sidecar still says `P30` while active run is newer.
9. **Package scope is too broad.** The archive includes `/Coding` root docs unrelated to Gloss, risking Codex contamination.
10. **Broad spec is far beyond current RC.** Treating all broad features as immediate work would hide release blockers.

## S1 high risks

- Existing notebooks need source health reconciliation after embedding/projection settings change.
- Pending summaries can still be confused with running summaries.
- Notes-only panel hides runtime proof; Inspector Dock is required for operator-grade debugging.
- Command logs may be excluded from packages if they remain `.log` files.
- Provider-specific decoding settings need exact support matrix and unsupported/opaque states.

## Release decision

No public release claim. No semantic-memory contribution claim. No TurboQuant contribution claim. Internal RC proof pass only.
