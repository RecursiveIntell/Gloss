# Changelog

## 0.2.0-alpha.1

- Changed `PolarCode` storage from unpacked angle indices to `packed_angle_indices`.
- Changed `QjlSketch` storage from unpacked signs to `packed_signs`.
- Added `bitpack`, `profile`, `codebook`, and `eval` modules.
- Added `CodecProfileV1`, `CompressionPolicyV1`, `CompressionReceiptV1`, and benchmark receipt types.
- Added `TurboMode` so QJL can be disabled by profile.
- Added asymmetric `KvQuantPolicy`, exact shadows, and KV score comparison.
- Added benchmark/profile/KV examples and P24 validation tests.
- Framed crate as experimental sidecar/shadow-mode infrastructure.
