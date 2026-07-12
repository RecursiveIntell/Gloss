# Changelog

## [Unreleased]
### Changed
- [LP-1] Per-request timeouts: moved timeout from `Client` builder to `RequestBuilder::timeout()` per-request
### Added
- `timeout` field on `LlmCall` for per-payload timeout overrides
- `request_timeout` field on `LlmRequest` for backend-level timeout propagation
