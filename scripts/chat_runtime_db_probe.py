#!/usr/bin/env python3
"""Inspect a Gloss app SQLite DB for provider/settings URL split-brain."""
from __future__ import annotations
import argparse, sqlite3, json
from pathlib import Path

URL_KEYS = ["ollama_url", "openai_base_url", "anthropic_base_url", "llamacpp_url"]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True, help="Path to Gloss app.db")
    args = ap.parse_args()
    path = Path(args.db)
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    settings = {r["key"]: r["value"] for r in conn.execute("SELECT key,value FROM settings WHERE key IN (%s)" % ",".join("?" for _ in URL_KEYS), URL_KEYS)}
    providers = [dict(r) for r in conn.execute("SELECT id, enabled, base_url, last_refreshed FROM providers ORDER BY id")]
    report = {"db": str(path), "settings_urls": settings, "providers": providers, "mismatches": []}
    mapping = {"ollama":"ollama_url", "openai":"openai_base_url", "anthropic":"anthropic_base_url", "llamacpp":"llamacpp_url"}
    for p in providers:
        key = mapping.get(p["id"])
        if not key: continue
        provider_url = (p.get("base_url") or "").rstrip('/')
        setting_url = (settings.get(key) or "").rstrip('/')
        if provider_url and setting_url and provider_url != setting_url:
            report["mismatches"].append({"provider": p["id"], "provider_base_url": provider_url, "setting_key": key, "setting_url": setting_url})
    print(json.dumps(report, indent=2, sort_keys=True))
    return 1 if report["mismatches"] else 0

if __name__ == "__main__":
    raise SystemExit(main())
