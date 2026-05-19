# P24 risk register

| Risk | Severity | Mitigation |
|---|---:|---|
| Overclaiming paper results as crate results | Critical | claim scanner, README rewrite, release gate |
| `encoded_bytes()` counts theoretical bits | Critical | packed storage tests, serialized byte tests |
| QJL hurts KV attention but remains default | High | `TurboMode`, KV policy, benchmark gate |
| Dense QR creates false production readiness | High | mark reference-only, add Hadamard/SRHT or remaining delta |
| Lloyd-Max is named but not implemented | High | codebook naming discipline |
| FibQuant gets merged into TurboQuant | High | interop docs, no path dependency |
| semantic-memory source-of-truth drift | Critical | sidecar-only docs and exact rerank requirement |
| weak tests pass despite broken math | High | metrics-based eval, benchmark receipts |
| release without dry-run proof | Critical | final gate |
| hidden cross-repo edits | High | fib-quant read-only inspection script |
