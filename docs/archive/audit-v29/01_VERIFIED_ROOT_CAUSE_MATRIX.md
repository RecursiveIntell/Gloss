# Verified Root-Cause Matrix

| Group | Verified? | User symptom it can explain | Core fix |
|---|---:|---|---|
| Source-list pre-send block | Yes | Send appears disabled / Ollama never called | Source status degrades retrieval only; chat still sends with `kind:none`. |
| Active notebook event filtering | Yes | Spinner remains after notebook switch or context mismatch | Route lifecycle events by active stream identity, not active notebook view. |
| Spawned task non-terminal returns | Yes | Spinner remains when cancellation/gate/notebook-switch branch happens | One terminal event for every spawned task exit. |
| Provider done-frame EOF wait | Mostly fixed, weakly tested | Old infinite stream bug | Add real fake-stream no-EOF regression test; do not refix unless it fails. |
| Assistant persistence failure hidden behind done | Yes | UI shows done but reload loses message / contradiction | `chat:done` only after durable assistant persistence or explicit partial/cancel artifact. |
| LAN Ollama rejected | Yes, conditional | Ollama works outside app but Gloss cannot reach LAN URL | Add explicit LAN opt-in only if trace proves LAN provider_config_error. |
| Stale/missing default model | Yes | Ollama reachable but selected model fails before generation | Validate default model against live model list; repair or block with clear UI. |
| Provider smoke not visible | Yes | Codex/user guesses at root cause | Expose smoke + last trace in Settings/Chat. |
| Package/run-truth drift | Yes | False release/pass claims | CurrentRunTruthV1 + package scope + final receipt gate. |
| Missing validation script | Yes | Fresh package cannot replay checks | Rename/allowlist validator and gate references to included files. |
