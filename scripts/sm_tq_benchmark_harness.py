#!/usr/bin/env python3
from __future__ import annotations
import argparse
import json
import math
from pathlib import Path


def top_k(values, k):
    return list(values or [])[:k]


def overlap(a, b, k):
    return len(set(top_k(a, k)) & set(top_k(b, k))) / max(1, min(k, len(a), len(b)))


def recall_proxy(exact, candidate, k):
    exact_k = set(top_k(exact, k))
    if not exact_k:
        return 1.0 if not candidate else 0.0
    return len(exact_k & set(top_k(candidate, k))) / len(exact_k)


def ndcg_proxy(exact, candidate, k):
    relevant = set(top_k(exact, k))
    if not relevant:
        return 1.0 if not candidate else 0.0

    def dcg(ranked):
        total = 0.0
        for idx, chunk_id in enumerate(top_k(ranked, k), start=1):
            if chunk_id in relevant:
                total += 1.0 / math.log2(idx + 1)
        return total

    ideal_len = min(k, len(relevant))
    ideal = sum(1.0 / math.log2(idx + 1) for idx in range(1, ideal_len + 1))
    return dcg(candidate) / ideal if ideal else 0.0


def count_markers(item, key):
    value = item.get(key, 0)
    if isinstance(value, bool):
        return 1 if value else 0
    if isinstance(value, int):
        return value
    if isinstance(value, list):
        return len(value)
    return 1 if value else 0


def latency_summary(rows, backend):
    values = []
    for row in rows:
        latency = (row.get("latency_ms") or {}).get(backend)
        if isinstance(latency, (int, float)):
            values.append(float(latency))
    if not values:
        return None
    values.sort()
    return {
        "count": len(values),
        "min_ms": values[0],
        "avg_ms": sum(values) / len(values),
        "max_ms": values[-1],
    }


def avg(rows, key):
    return sum(row[key] for row in rows) / len(rows) if rows else 0.0

def main() -> int:
    ap = argparse.ArgumentParser(description="Compare exact, semantic-memory, and TurboQuant result lists from a JSON fixture/report.")
    ap.add_argument("--input", help="JSON file with items: [{query, exact:[ids], semantic:[ids], turbo:[ids]}]")
    ap.add_argument("--output", help="Optional path for the JSON report.")
    ap.add_argument("--k", type=int, default=10)
    ap.add_argument("--min-overlap", type=float, default=0.8)
    ap.add_argument("--template", action="store_true")
    args = ap.parse_args()
    if args.template or not args.input:
        print(json.dumps({
            "format": [{
                "query": "example",
                "exact": ["chunk1", "chunk2"],
                "semantic": ["chunk1", "chunk2"],
                "turbo": ["chunk1", "chunk3"],
                "latency_ms": {"exact": 0, "semantic": 0, "turbo": 0},
                "source_scope_violations": [],
                "missing_backpointers": [],
                "stale_candidates": [],
                "fallback_reason": None
            }],
            "note": "Populate this from captured Gloss/semantic-memory exact and TurboQuant runs. Latency is reported only when present in the input."
        }, indent=2))
        return 0
    data = json.loads(Path(args.input).read_text())
    rows = []
    for item in data:
        ex = item.get("exact", [])
        sm = item.get("semantic", item.get("semantic_memory", []))
        tq = item.get("turbo", [])
        fallback_reason = item.get("fallback_reason") or item.get("semantic_memory_fallback_reason")
        row = {
            "query": item.get("query"),
            "exact_count": len(ex),
            "semantic_count": len(sm),
            "turbo_count": len(tq),
            "semantic_overlap_at_k": overlap(ex, sm, args.k),
            "turbo_overlap_at_k": overlap(ex, tq, args.k),
            "semantic_recall_proxy_at_k": recall_proxy(ex, sm, args.k),
            "turbo_recall_proxy_at_k": recall_proxy(ex, tq, args.k),
            "semantic_ndcg_proxy_at_k": ndcg_proxy(ex, sm, args.k),
            "turbo_ndcg_proxy_at_k": ndcg_proxy(ex, tq, args.k),
            "source_scope_violation_count": count_markers(item, "source_scope_violations"),
            "missing_backpointer_count": count_markers(item, "missing_backpointers"),
            "stale_candidate_count": count_markers(item, "stale_candidates"),
            "fallback_reason": fallback_reason,
            "latency_ms": item.get("latency_ms") or {},
        }
        rows.append(row)

    fallback_count = sum(1 for row in rows if row["fallback_reason"])
    source_scope_violations = sum(row["source_scope_violation_count"] for row in rows)
    missing_backpointers = sum(row["missing_backpointer_count"] for row in rows)
    stale_candidates = sum(row["stale_candidate_count"] for row in rows)
    out = {
        "ok": bool(rows)
        and avg(rows, "semantic_overlap_at_k") >= args.min_overlap
        and avg(rows, "turbo_overlap_at_k") >= args.min_overlap
        and source_scope_violations == 0
        and missing_backpointers == 0
        and stale_candidates == 0,
        "k": args.k,
        "min_overlap": args.min_overlap,
        "query_count": len(rows),
        "avg_semantic_overlap_at_k": avg(rows, "semantic_overlap_at_k"),
        "avg_turbo_overlap_at_k": avg(rows, "turbo_overlap_at_k"),
        "avg_semantic_recall_proxy_at_k": avg(rows, "semantic_recall_proxy_at_k"),
        "avg_turbo_recall_proxy_at_k": avg(rows, "turbo_recall_proxy_at_k"),
        "avg_semantic_ndcg_proxy_at_k": avg(rows, "semantic_ndcg_proxy_at_k"),
        "avg_turbo_ndcg_proxy_at_k": avg(rows, "turbo_ndcg_proxy_at_k"),
        "fallback_rate": fallback_count / len(rows) if rows else 0.0,
        "source_scope_violation_count": source_scope_violations,
        "missing_backpointer_count": missing_backpointers,
        "stale_candidate_count": stale_candidates,
        "latency_ms": {
            "exact": latency_summary(rows, "exact"),
            "semantic": latency_summary(rows, "semantic"),
            "turbo": latency_summary(rows, "turbo"),
        },
        "rows": rows,
    }
    rendered = json.dumps(out, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).parent.mkdir(parents=True, exist_ok=True)
        Path(args.output).write_text(rendered + "\n")
    print(rendered)
    return 0 if out["ok"] else 1

if __name__ == '__main__':
    raise SystemExit(main())
