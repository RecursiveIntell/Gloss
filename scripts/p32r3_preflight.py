#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, shutil, subprocess, time
from pathlib import Path

PRIMARY = [
    'src-tauri/src/commands/chat.rs',
    'src-tauri/src/commands/sources.rs',
    'src-tauri/src/commands/settings.rs',
    'src-tauri/src/providers/mod.rs',
    'src-tauri/src/providers/ollama.rs',
    'src-tauri/src/jobs/mod.rs',
    'src-tauri/src/lib.rs',
    'src-tauri/src/db/notebook_db.rs',
    'src/lib/events.ts',
    'src/lib/types.ts',
    'src/lib/tauri.ts',
    'src/stores/sourceStore.ts',
    'src/components/chat/ChatPanel.tsx',
    'src/components/sources/SourcesPanel.tsx',
    'src/components/layout/StatusBar.tsx',
]


def run(cmd: str) -> dict:
    try:
        p = subprocess.run(cmd, shell=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=20)
        return {'cmd': cmd, 'code': p.returncode, 'out': p.stdout[-4000:]}
    except Exception as e:
        return {'cmd': cmd, 'code': 999, 'out': str(e)}


def validate_skill_layout(root: Path) -> list[str]:
    errors: list[str] = []
    skills_root = root / '.agents' / 'skills'
    if not skills_root.exists():
        errors.append('.agents/skills missing')
        return errors
    for skill in sorted(skills_root.glob('*/SKILL.md')):
        txt = skill.read_text(encoding='utf-8', errors='replace')
        if not txt.startswith('---'):
            errors.append(f'{skill.relative_to(root)}: missing YAML front matter')
            continue
        parts = txt.split('---', 2)
        meta = parts[1] if len(parts) >= 3 else ''
        if 'name:' not in meta or 'description:' not in meta:
            errors.append(f'{skill.relative_to(root)}: missing name/description')
    return errors


def validate_agent_configs(root: Path) -> list[dict]:
    errors: list[dict] = []
    try:
        import tomllib
    except Exception:
        return [{'error': 'tomllib unavailable'}]
    agents_root = root / '.codex' / 'agents'
    for agent in sorted(agents_root.glob('*.toml')):
        data = tomllib.loads(agent.read_text(encoding='utf-8'))
        missing = [k for k in ['name', 'description', 'developer_instructions'] if not data.get(k)]
        if missing:
            errors.append({'path': str(agent.relative_to(root)), 'missing': missing})
    return errors


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--run-id', default='P32R3')
    args = ap.parse_args()
    root = Path.cwd()
    out = root / 'docs' / 'codex-runs' / args.run_id / 'receipts'
    out.mkdir(parents=True, exist_ok=True)
    missing = [p for p in PRIMARY if not (root / p).exists()]
    tools = {t: shutil.which(t) for t in ['git', 'python3', 'node', 'npm', 'cargo', 'ollama', 'jq']}
    skill_errors = validate_skill_layout(root)
    agent_errors = validate_agent_configs(root)
    rec = {
        'run_id': args.run_id,
        'ts': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
        'root': str(root),
        'missing_primary_files': missing,
        'tools': tools,
        'git_status_before': run('git status --short'),
        'git_branch': run('git branch --show-current'),
        'skill_layout_errors': skill_errors,
        'agent_config_errors': agent_errors,
    }
    (out / 'preflight.json').write_text(json.dumps(rec, indent=2, sort_keys=True), encoding='utf-8')
    (out / 'git_status_before.txt').write_text(rec['git_status_before']['out'], encoding='utf-8')
    if missing or skill_errors or agent_errors:
        print('Preflight failures:', {'missing_primary_files': missing, 'skill_layout_errors': skill_errors, 'agent_config_errors': agent_errors})
        return 2
    print(json.dumps({'ok': True, 'receipt': str(out / 'preflight.json')}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
