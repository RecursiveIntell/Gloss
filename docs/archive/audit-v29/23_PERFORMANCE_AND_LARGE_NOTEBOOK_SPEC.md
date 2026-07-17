# Performance and Large Notebook Spec

## RC performance targets

- smooth UI with current 100–300 displayed source cap;
- clear pending vs processing queue counts;
- no massive UUID flood in evidence panel;
- strict fixture latency recorded;
- local generation timeout/continuation reliable.

## Broad targets after RC

| Scale | Gate |
|---|---|
| 200 sources | import, source list, retrieval, backfill receipts |
| 2,000 sources | virtualized source list or capped grouped display; queue truth; no UI lock |
| 10k chunks | dense index build/search timing; FTS search timing; semantic projection timing |
| cold start | app opens without eager expensive rebuilds |
| backfill | cancel/resume safe |

## Required metrics

- import duration;
- chunk count;
- embed throughput;
- projection throughput;
- retrieval latency p50/p95;
- answer time-to-first-token;
- stream idle events;
- UI render budget.
