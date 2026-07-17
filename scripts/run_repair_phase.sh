#!/usr/bin/env bash
# Run a single Gloss repair phase via codex exec. Captures the last-message
# to docs/codex-runs/GLOSS_TOTAL_RUNTIME_REPAIR_20260612/receipts/<phase>.codex.txt
# and the last status JSONL to <phase>.codex.jsonl so the orchestrator can
# verify the agent actually exited with the work done.
set -euo pipefail

PHASE_FILE="$1"
PHASE_ID="$2"
ROOT="${3:-/home/sikmindz/Coding/Gloss}"
RECEIPT_DIR="$ROOT/docs/codex-runs/GLOSS_TOTAL_RUNTIME_REPAIR_20260612/receipts"
PHASE_PROMPT_FILE="$RECEIPT_DIR/PHASE_${PHASE_ID}_PROMPT.md"
OUT_MSG="$RECEIPT_DIR/PHASE_${PHASE_ID}.codex.txt"
OUT_JSONL="$RECEIPT_DIR/PHASE_${PHASE_ID}.codex.jsonl"
SUMMARY="$RECEIPT_DIR/PHASE_${PHASE_ID}.SUMMARY.md"

cd "$ROOT"

# Build a self-contained codex prompt for this phase. The bundle's phase
# markdown is the canonical task spec; the orchestrator's preflight
# supplies the baseline gate failures and the strict success contract.
cat > "$PHASE_PROMPT_FILE" <<EOF
You are Codex working on the Gloss repo at $ROOT.

## Authority order
1. Current source files in the checked-out Gloss repo at $ROOT.
2. The Gloss Repair Bundle at /tmp/gloss_bundle/gloss_repair_codex_bundle_20260612/
   (phases/PHASE_${PHASE_ID}_*.md is YOUR canonical task spec; read it first).
3. Existing active repo docs/validators (validation/run_all_gloss_repair_gates.sh).
4. Prior audit docs (docs/codex-runs/...).

Source code beats prose. If the prompt and current code disagree, inspect
and patch the code path, then record the divergence.

## Hard requirements (apply to every phase)
- No silent fallback. If retrieval/projection/provider/semantic-memory
  degrades, surface a typed reason and receipt.
- No hidden provider work. Timeouts/cancel must cancel the underlying
  provider task/request, not just UI futures.
- No event-only truth. Chat UI must recover from DB/attempt state after
  listener loss, remount, focus, HMR, or terminal event miss.
- No orphan semantic-memory links. Links must point to real Gloss chunks
  or use a schema that explicitly models parent chunk/subchunk identity.
- No hardcoded embedding dimensions for user-selected Ollama models.
  Probe or use validated model metadata.
- No settings lies. Settings diagnostics must reflect actual configured
  provider/model/dim/status, not defaults.
- No terminal state contradictions. Attempt trace, DB, and UI must
  converge for success, timeout, cancellation, and error.
- No uncontrolled concurrent LLM generation. Enforce single-flight or
  explicit cancel/replace.
- No compatibility shim hiding an async/sync runtime boundary bug.
  Bridges may be temporary only if tested and ticketed; prefer async
  end-to-end.

## Forbidden leftovers (still forbidden after your edits)
- provider.chat(request) call sites without execution context.
- tokio::time::timeout(provider.chat(...)) as the only timeout mechanism.
- reqwest::blocking::Client in runtime paths.
- config.embedding.dimensions = defaults.dimensions for semantic-memory Ollama.
- Synthetic semantic-memory subchunk IDs as primary chunk_id unless
  schema + DB doctor understand them.
- Settings diagnostics that hardcode semantic-memory dimensions/model.
- Chat terminal handling that only appends in-memory streamed content
  and never reconciles DB.
- Failed/cancelled attempts with no durable terminal status.
- Broad background job cancellation that kills unrelated work.
- New fallback reason strings without typed enum/test coverage.

## Phase task spec (CANONICAL)
Read and follow exactly:
  /tmp/gloss_bundle/gloss_repair_codex_bundle_20260612/phases/PHASE_${PHASE_ID}_*.md

That file is your task spec. Do not skip its required commands or
deliverables. Write the deliverable to
  $RECEIPT_DIR/PHASE_${PHASE_ID}.md

## Required validation (run after your edits)
From $ROOT:

\`\`\`bash
cargo fmt --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
npm run build
npm test
python3 validation/gloss_runtime_static_gate.py .
python3 validation/gloss_provider_cancellation_static_gate.py .
python3 validation/gloss_semantic_memory_contract_gate.py .
python3 validation/gloss_settings_contract_gate.py .
python3 validation/gloss_receipt_consistency_gate.py .
bash validation/run_all_gloss_repair_gates.sh .
\`\`\`

If a command cannot run, record the blocker exactly in your SUMMARY and
run the static gates anyway.

## Output requirements
1. Edit source files in the repo to satisfy the phase spec.
2. Write $RECEIPT_DIR/PHASE_${PHASE_ID}.md with:
   - phase name,
   - changed files,
   - commands run,
   - pass/fail/skipped status with exact output snippets,
   - runtime scenarios exercised,
   - unresolved risks,
   - rollback point (which commit to git revert if needed).
3. Write $SUMMARY with a 5-15 line plain-text wrap-up: what was done,
   what passes, what doesn't, blockers, manual verification commands.

## Working constraints
- Do NOT touch the 3 prior-run owned dirty files
  (docs/codex-runs/CODEX_RUN_INDEX.md, CURRENT_RUN.md,
  scripts/gloss_p36_perf_probe.py).
- Do NOT add or remove dependencies in Cargo.toml/package.json without
  recording the change with a justification in the receipt.
- Do NOT delete existing tests; extend them.
- Do NOT create new shadow truth stores.
- Do NOT mark anything done without a passing command output. Receipts
  or it did not happen.

Begin.
EOF

echo "codex exec starting: phase=$PHASE_ID"
codex exec \
  --sandbox workspace-write \
  -C "$ROOT" \
  --ephemeral \
  --json \
  "$(cat "$PHASE_PROMPT_FILE")" \
  > "$OUT_JSONL" 2>&1 &
CODEX_PID=$!

# Wait for codex to exit, but cap at 30 minutes.
WAITED=0
MAX_WAIT=1800
while kill -0 "$CODEX_PID" 2>/dev/null; do
  sleep 5
  WAITED=$((WAITED+5))
  if [ "$WAITED" -ge "$MAX_WAIT" ]; then
    echo "TIMEOUT: codex did not exit within $MAX_WAIT s" >&2
    kill -TERM "$CODEX_PID" 2>/dev/null || true
    sleep 2
    kill -KILL "$CODEX_PID" 2>/dev/null || true
    exit 2
  fi
done

# Extract the last assistant message from the JSONL stream.
python3 - "$OUT_JSONL" "$OUT_MSG" <<'PY'
import json, sys
src, dst = sys.argv[1], sys.argv[2]
last = None
with open(src, errors="ignore") as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except Exception:
            continue
        if obj.get("type") in ("assistant_message", "message", "final_message"):
            last = obj
if last is None:
    with open(dst, "w") as f:
        f.write("(no assistant message in JSONL)\n")
else:
    txt = last.get("message") or last.get("content") or last
    if isinstance(txt, list):
        txt = "".join(p.get("text", "") if isinstance(p, dict) else str(p) for p in txt)
    with open(dst, "w") as f:
        f.write(str(txt) + "\n")
PY

echo "codex exec finished: phase=$PHASE_ID pid=$CODEX_PID waited=${WAITED}s"
echo "  summary: $SUMMARY"
echo "  last message: $OUT_MSG"
echo "  jsonl: $OUT_JSONL"
exit 0
