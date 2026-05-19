#!/usr/bin/env python3
"""Stop gate reminder. This hook is advisory unless the Codex hook runner treats nonzero as blocking."""
from pathlib import Path
import sys
repo = Path.cwd()
run = repo / 'docs/codex-runs/CHAT_RUNTIME_FIX_20260518'
if not run.exists():
    print('CHAT_RUNTIME_STOP_GATE: run directory missing; create docs/codex-runs/CHAT_RUNTIME_FIX_20260518', file=sys.stderr)
    sys.exit(1)
sys.exit(0)
