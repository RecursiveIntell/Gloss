# Phase 03 — Packaging Policy Repair

Validation command:

```bash
python scripts/zip_source_certifier.py --mode next-codex-context --no-archive-codex-runs --no-strict --output /tmp/scr-runtime-phase-03-packaging.zip && \
python scripts/assert_archive_includes_codex.py /tmp/scr-runtime-phase-03-packaging.zip && \
rm -f /tmp/scr-runtime-phase-03-packaging.zip
```
