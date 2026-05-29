# TurboQuant Proof and Benchmark Spec

## RC proof requirement

Gloss may only claim TurboQuant contributes if a live fixture proves:

- TurboQuant feature compiled;
- runtime setting enabled;
- semantic-memory store builds vector/TurboQuant artifacts;
- artifact digest and generation id recorded;
- exact rerank is required and performed;
- exact_rerank_count > 0;
- answer/retrieval receipt links to the artifact.

If this cannot be proven in the RC pass, demote claim to:

```text
TurboQuant support is compiled/configured but live contribution is not release-proven.
```

## Benchmark follow-up

Benchmark after RC:

- corpus: small fixture, medium 10k chunk fixture, large synthetic;
- metrics: latency, recall@k proxy, exact rerank count, artifact build time, memory use;
- compare: native dense/hybrid vs semantic-memory vs semantic-memory+TQ;
- no superiority claim until reproduced.
