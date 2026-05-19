#!/usr/bin/env python3
from __future__ import annotations
import argparse, json, sqlite3, sys
from pathlib import Path

QUERIES = {
    'bad_synced_links': """
        SELECT COUNT(*) FROM semantic_memory_links
        WHERE sync_status = 'synced'
          AND (sm_document_id IS NULL OR sm_chunk_id IS NULL OR content_digest IS NULL)
    """,
    'synced_rows': "SELECT COUNT(*) FROM semantic_memory_links WHERE sync_status='synced'",
    'degraded_rows': "SELECT COUNT(*) FROM semantic_memory_links WHERE sync_status LIKE 'degraded%' OR sync_status='degraded'",
}

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--db', required=True, help='Path to Gloss notebook SQLite DB')
    args = ap.parse_args()
    db = Path(args.db).expanduser().resolve()
    if not db.exists():
        print(json.dumps({'ok': False, 'error': f'missing db: {db}'}))
        return 2
    conn = sqlite3.connect(str(db))
    results = {}
    try:
        for name, q in QUERIES.items():
            results[name] = conn.execute(q).fetchone()[0]
    except Exception as e:
        print(json.dumps({'ok': False, 'error': str(e), 'db': str(db)}, indent=2))
        return 2
    ok = results.get('bad_synced_links') == 0
    print(json.dumps({'ok': ok, 'db': str(db), 'results': results}, indent=2, sort_keys=True))
    return 0 if ok else 1

if __name__ == '__main__':
    raise SystemExit(main())
