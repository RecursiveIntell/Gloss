---
name: chat-runtime-diagnostics
description: Diagnose and repair Gloss chat no-response failures with durable traces, provider smoke, and frontend event routing gates.
---

Use when a Gloss chat prompt fails to visibly respond.

Checklist:
1. Run `python3 scripts/chat_runtime_static_audit.py --repo .`.
2. Inspect `src/App.tsx` chat event routing.
3. Inspect provider config paths in settings and provider registry.
4. Add or verify `ChatAttemptTraceV1`.
5. Add or verify provider-only smoke.
6. Ensure semantic-memory fallback cannot block chat when enabled.
7. Produce final receipt.
