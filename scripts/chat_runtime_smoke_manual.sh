#!/usr/bin/env bash
set -euo pipefail
cat <<'EOF'
Manual Gloss Chat Runtime Smoke

Run from repo root after building/launching Gloss.

1. Set memory_backend=gloss-local and memory_backend_fallback=true.
2. Configure Ollama provider URL in Settings.
3. Click Test Provider. It must use the same URL shown in diagnostics.
4. Refresh models. Confirm cogito:3b and qwen3.5:4b are available.
5. Send: Reply exactly: gloss smoke ok
6. Expected: visible streaming tokens or visible provider error.
7. Open/copy last ChatAttemptTraceV1.
8. Confirm assistant message persisted in conversation.
9. Repeat for qwen3.5:4b with cold-load timeout budget.
10. Set semantic-memory-preview with invalid embedding URL and fallback=true.
11. Repeat prompt. Expected: fallback/degradation visible and provider still streams.
12. Set fallback=false. Expected: visible semantic-memory error, not silent no-response.

Save outputs under docs/codex-runs/CHAT_RUNTIME_FIX_20260518/.
EOF
