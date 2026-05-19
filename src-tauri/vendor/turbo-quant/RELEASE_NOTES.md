# Release Notes: 0.2.0-alpha.1

This alpha is a storage and API break from `0.1.0`.

The release candidate introduces packed Polar/QJL payloads, explicit codec profiles, compression receipts, optional QJL mode, asymmetric KV policies, and benchmark receipt generation. It is suitable for local experiments and sidecar/shadow-mode integration work.

Do not present this crate as production KV-cache infrastructure. Do not use compressed codes as the source of truth for retrieval or evidence. Downstream systems should retain exact vectors and benchmark exact-vs-compressed behavior before promotion.
