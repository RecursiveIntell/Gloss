# Push / Release Decision

## Decision

**NO-GO.** Do not push Gloss as release-ready or public-ready from this package.

## Blocking reasons

1. Chat can fail to terminalize correctly because provider `done=true` is not treated as immediate completion.
2. Partial outputs are not proven durable on timeout/error/cancel.
3. Package scope gate fails from the transferred context.
4. Release candidate gate can hang instead of emitting structured failure.
5. Desktop smoke is contract-only and explicitly not release-grade.
6. Rust and live Ollama validation were not runnable in this sandbox.
7. TurboQuant exact runtime proof remains missing.
8. Secret scan/filename exclusion policy has unresolved warnings and removes required proof artifacts.

## Allowed positioning until fixed

- Local-first Gloss prototype/workbench.
- Evidence/provenance-oriented notebook/RAG implementation under active hardening.
- Frontend build/tests pass in this audit environment.

## Blocked claims until fixed

- Release-ready.
- Fully validated desktop app.
- Live Ollama chat path proven stable.
- Complete broad import support.
- TurboQuant runtime advantage/proof.
- Security/compliance maturity.
