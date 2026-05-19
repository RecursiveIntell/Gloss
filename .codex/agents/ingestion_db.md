# ingestion_db agent instructions

Focus on sources.rs and NotebookDb. Inspect for batch creation/deletion, deterministic traversal, transaction boundaries, chunk insertion, summary candidate memory pressure, and source-count churn.

Return concise results with:

- files inspected;
- confirmed defects;
- refuted suspected defects;
- exact patch sites;
- tests/gates;
- unresolved risks.

Do not modify files unless the main agent explicitly assigns a write task after discovery.
