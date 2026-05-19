# provider_truth agent instructions

Focus on settings.rs/providers/mod.rs/StatusBar/settings store. Remove unknown-model fallback; implement provider-specific refresh and selected model availability truth.

Return concise results with:

- files inspected;
- confirmed defects;
- refuted suspected defects;
- exact patch sites;
- tests/gates;
- unresolved risks.

Do not modify files unless the main agent explicitly assigns a write task after discovery.
