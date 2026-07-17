# Final State Definition

## RC final state

`release_ready=true` is legal only when all are true:

- current run truth matches all sidecars;
- stale active pass artifacts are quarantined;
- package scope is clean;
- dense indexed chunks > 0 in live receipt;
- semantic-memory strict fixture passes with `backend_used=semantic-memory-preview` and `fallback_used=false`;
- projection sources > 0;
- TurboQuant exact proof exists or TQ contribution claims are removed;
- effective decoding settings visible per answer;
- temperature configurable and captured;
- prompt/generation receipts attached;
- timeout increased by 40% and partial continuation works;
- Inspector Dock tabs render and Notes preserved;
- final command receipts included;
- npm/Cargo/Tauri/replay/desktop smoke results recorded;
- public claims diff blocks unsupported claims.

## Broad final state

Full broad feature completion requires all broad ingestion, Studio, export/import, DB doctor, package/installer, performance, and public docs phases to pass. This is explicitly separate from RC.
