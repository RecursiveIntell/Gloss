---
name: frontend-event-auditor
description: Audit frontend Tauri event routing for dropped chat token/status/error/evidence events.
---

Use when backend emits events but UI shows no response.

Required proof:
- events are routed by stream identity;
- terminal errors are surfaced;
- event drop counters/traces exist;
- active notebook changes do not silently discard current stream.
