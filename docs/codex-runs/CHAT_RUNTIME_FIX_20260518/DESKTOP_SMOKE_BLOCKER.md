# Desktop Smoke Blocker

Date: 2026-05-18

The live desktop smoke gate was not completed from this shell.

Observed environment:
- `DISPLAY=:0`
- `WAYLAND_DISPLAY=wayland-0`
- `XDG_SESSION_TYPE=wayland`
- Local Ollama is reachable and provider-only smoke passed for `cogito:3b` and `qwen3.5:4b`.

Missing automation/capture tools:
- `tauri-driver` not found in PATH or common local locations.
- `xvfb-run` not found.
- `gnome-screenshot` not found.
- `grim` not found.

Bounded launch attempt:
- `timeout 90 npm run tauri dev 2>&1 | tee docs/codex-runs/CHAT_RUNTIME_FIX_20260518/tauri_dev_launch_attempt.log`
- Result: Tauri dev compiled and ran `target/debug/gloss`.
- Result: backend logged `Gloss initialized` and `Summary job loop started`.
- Blocker: Vite/esbuild reported `The service is no longer running: write EPIPE` before any controlled chat interaction or screenshot/event capture.

Reason this blocks certification:
- Launching the Tauri app alone is not enough to prove chat response visibility.
- The required gate needs UI event counters, backend logs, screenshot/recording notes, visible streamed-token/error proof, and/or persisted assistant-message proof from the actual desktop runtime.
- Without a WebDriver bridge or screenshot/control harness, I cannot honestly certify the live desktop UI path from this environment.

Required next proof:
- Run the manual desktop smoke checklist in `scripts/chat_runtime_smoke_manual.sh`.
- Capture `ChatAttemptTraceV1`, backend logs, UI event counters, screenshot/recording notes, and persisted assistant-message or visible-error proof for:
  - `memory_backend=gloss-local` + `cogito:3b`
  - `memory_backend=gloss-local` + `qwen3.5:4b`
  - semantic-memory-preview with bad embedding URL + fallback enabled
  - semantic-memory-preview with bad embedding URL + fallback disabled
