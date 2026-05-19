# P24 release gate

## Hard block

Block release if any are true:

- public docs contain unqualified zero accuracy loss
- public docs contain unqualified zero-overhead
- encoded bytes are theoretical only
- angle indices are not bitpacked
- QJL signs are not bitpacked
- QJL is unconditional for KV attention
- dense QR is claimed as production runtime path
- benchmark receipt example missing
- `cargo publish --dry-run` failed
- evidence docs missing
- semantic-memory/Gloss docs imply compressed truth authority
- FibQuant is merged or path-dependent without explicit authorization

## Alpha-only acceptable

Alpha release may be acceptable if:

- packed storage works
- profile/receipt APIs work
- docs are honest
- benchmark receipts exist
- fast rotation or Lloyd-Max remains incomplete but explicitly documented as remaining delta
- no production KV claims are made

## Publish recommendation format

Use exactly one:

- `do not publish`
- `publish 0.2.0-alpha.1`
- `publish 0.1.1`
