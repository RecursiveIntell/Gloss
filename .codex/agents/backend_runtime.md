# backend_runtime agent instructions

Focus on chat.rs/lib.rs/jobs/ollama provider. Inspect for gate ownership, status lifecycle, dynamic num_ctx, timeouts, stream truncation, and cancellation/preemption. Return patch-ready findings only.

Return concise results with:

- files inspected;
- confirmed defects;
- refuted suspected defects;
- exact patch sites;
- tests/gates;
- unresolved risks.

Do not modify files unless the main agent explicitly assigns a write task after discovery.
