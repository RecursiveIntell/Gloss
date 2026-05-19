# Retrieval Benchmark Gates

## Smoke Gate

- dim: 384
- corpus: 1k
- queries: 50
- recall@10 >= 0.99
- ndcg@10 >= 0.99
- encoded bytes/vector < raw bytes/vector

## Internal Gate

- dims: 384, 768, 1536
- corpus: 10k
- queries: at least 100
- recall@10 >= 0.98
- ndcg@10 >= 0.98
- p95 candidate latency budget recorded
- exact rerank count recorded
- encoded bytes/vector < raw bytes/vector

## Release-Candidate Gate

- dims: 384, 768, 1536
- corpus: 100k
- filtered and unfiltered queries
- stale/corrupt artifact scenarios
- rebuild time
- memory footprint
- p50/p95/p99 latency
- exact baseline comparison
- regression threshold from previous accepted run

## Default-Eligibility Gate

Defined but not required for P32. It requires release-candidate gate success, no unexplained fallback, no proof-debted workspace gates, and an explicit operator decision to enable TurboQuant outside the feature-gated lane.
