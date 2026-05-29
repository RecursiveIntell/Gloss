# Phase 04 — Validation Gate Hardening

Tasks:
1. Fix package scope gate.
2. Include/rename missing secret-store permissions gate.
3. Add subgate timeouts to release candidate gate.
4. Fail release gate on `release_blocker=true`.
5. Strengthen static chat audits.

Acceptance:
- every gate emits structured JSON;
- aggregate release gate never hangs;
- current known blockers fail gates until fixed.
