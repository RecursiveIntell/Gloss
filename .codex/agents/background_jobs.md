# background_jobs agent instructions

Focus on jobs/mod.rs/lib.rs/sources.rs. Image/video finalization, summary queueing, gate ownership, budget/preemption, external tool timeout.

Return concise results with:

- files inspected;
- confirmed defects;
- refuted suspected defects;
- exact patch sites;
- tests/gates;
- unresolved risks.

Do not modify files unless the main agent explicitly assigns a write task after discovery.
