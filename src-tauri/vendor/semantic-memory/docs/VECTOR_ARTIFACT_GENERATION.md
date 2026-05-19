# Vector Artifact Generation

P32 introduces generation-level manifests for derived vector artifacts.

## Contract

`DerivedVectorArtifactGenerationV1` records:

- `generation_id`
- `codec_family`
- `codec_profile_digest`
- `source_snapshot_digest`
- `source_row_count`
- `artifact_count`
- `source_tables`
- `dim`
- `encoding`
- `created_at`
- `build_receipt_id`
- `artifact_manifest_digest`
- `status`
- `degradations`

## Persistence

The SQLite table is `derived_vector_artifact_generations`. Active artifacts reference `generation_id` in `derived_vector_artifacts`.

Rebuild behavior:

- supersedes prior active generations for the same codec/profile;
- writes a new generation row;
- writes active artifacts for that generation;
- returns a build receipt containing generation and manifest digests.

Invalidation behavior:

- authoritative embedding writes, re-embeds, and deletes mark active generations invalidated;
- stale generations are not silently used;
- rebuild repairs invalidation by producing a fresh active generation.
