# P24 storage and wire-format target

## Problem

Current storage may use wider Rust containers while public docs describe sub-byte compression. P24 must make storage accounting real.

## Packed indices

Angle indices:

- valid bits: 1..=16
- index must be `< 2^bits`
- packed little-endian bit order
- serialized as `Vec<u8>`
- count and bit width must be included in code/profile

## Packed signs

QJL signs:

- logical values: -1 or +1
- packed as bits
- sign convention documented:
  - `0 => -1`
  - `1 => +1`
- projection count included in code/profile

## encoded_bytes

Must count serialized packed payload, including necessary metadata if the code object claims serialized size.

Do not count:
- ideal mathematical bit count only
- heap capacity
- Rust struct overhead unless explicitly labeled as in-memory overhead

## Receipts

Every receipt should include:

- encoded bytes
- fp16 baseline bytes
- fp32 baseline bytes
- compression ratios
- profile digest
- storage layout
