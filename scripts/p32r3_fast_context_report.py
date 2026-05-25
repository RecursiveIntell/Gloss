#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, os, subprocess, time
from pathlib import Path

WATCH = [
 'src-tauri/src/commands/chat/mod.rs','src-tauri/src/commands/sources/mod.rs','src-tauri/src/commands/settings.rs',
 'src-tauri/src/providers/mod.rs','src-tauri/src/providers/ollama.rs','src-tauri/src/jobs/mod.rs','src-tauri/src/lib.rs',
 'src-tauri/src/db/notebook_db/mod.rs','src/lib/events.ts','src/lib/types.ts','src/lib/tauri.ts',
 'src/stores/sourceStore.ts','src/components/chat/ChatPanel.tsx','src/components/sources/SourcesPanel.tsx','src/components/layout/StatusBar.tsx'
]
PATTERNS = ['num_ctx: Some(16_384)', 'get_provider_for_model', 'refresh_models', 'list_sources_needing_summary', 'walk_directory_inner', 'setSelectedSources', 'onChatToken']

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--run-id', default='P32R3'); args=ap.parse_args()
    root=Path.cwd(); report=root/'docs'/'codex-runs'/args.run_id/'reports'/'FAST_CONTEXT_REPORT.md'; report.parent.mkdir(parents=True, exist_ok=True)
    lines=[f'# {args.run_id} fast context report', '', f'Generated: {time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}', '']
    for rel in WATCH:
        p=root/rel
        lines.append(f'## {rel}')
        if not p.exists(): lines.append('MISSING'); continue
        txt=p.read_text(errors='ignore')
        lines.append(f'- bytes: {p.stat().st_size}')
        for pat in PATTERNS:
            if pat in txt: lines.append(f'- contains `{pat}`')
        lines.append('')
    report.write_text('\n'.join(lines), encoding='utf-8')
    print(report)
if __name__ == '__main__': raise SystemExit(main())
