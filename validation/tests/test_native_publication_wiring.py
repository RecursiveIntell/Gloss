"""Source wiring guards supplement (not replace) actual DB/dense native tests."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


def check_wiring(state: str, sources: str) -> list[str]:
    errors = []
    cache = state.split("pub fn ensure_hnsw_index(", 1)[1].split("pub fn save_hnsw_index(", 1)[0]
    cleanup = state.split("pub fn save_hnsw_index(", 1)[1].split("// --- Scheduling helpers ---", 1)[0]
    if "load_published_dense_index" not in cache:
        errors.append("cache must use read-only published loader")
    if "publish_dense_cleanup" not in cleanup:
        errors.append("cleanup must validate surviving mappings")
    for body in (cache, cleanup):
        if "upsert_embedding_index_metadata" in body or "mark_embedding_index_status" in body:
            errors.append("cache/cleanup must not rewrite publication metadata")
    for name, commit in [
        ("pub async fn delete_source(", "delete_source_with_projection_invalidation"),
        ("fn delete_source_ids_for_notebook(", "delete_source_with_projection_invalidation"),
        ("pub async fn retry_source_ingestion(", "reset_source_for_reingestion"),
    ]:
        body = sources.split(name, 1)[1].split("\n}", 1)[0]
        canonical = body.find(commit)
        projection = body.find("if !old_embedding_ids.is_empty()")
        if canonical < 0 or projection < 0 or canonical > projection:
            errors.append(f"{name}: canonical commit must precede projection removal")
    return errors


class NativePublicationWiringTests(unittest.TestCase):
    def setUp(self):
        self.state = (ROOT / "src-tauri/src/state.rs").read_text()
        self.sources = (ROOT / "src-tauri/src/commands/sources/mod.rs").read_text()

    def test_current_call_sites_preserve_owner_boundaries(self):
        self.assertEqual(check_wiring(self.state, self.sources), [])

    def test_rejects_cache_readiness_rewrite(self):
        changed = self.state.replace("pub fn ensure_hnsw_index(",
            "pub fn ensure_hnsw_index( /* upsert_embedding_index_metadata */", 1)
        self.assertTrue(check_wiring(changed, self.sources))

    def test_rejects_projection_removal_before_canonical_commit(self):
        changed = self.sources.replace("db.delete_source_with_projection_invalidation",
            "db.unchecked_delete", 1)
        self.assertTrue(check_wiring(self.state, changed))
