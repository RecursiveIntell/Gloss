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
    ]:
        body = sources.split(name, 1)[1].split("\n}", 1)[0]
        canonical = body.find(commit)
        projection = body.find("if !old_embedding_ids.is_empty()")
        if canonical < 0 or projection < 0 or canonical > projection:
            errors.append(f"{name}: canonical commit must precede projection removal")
    retry_claim = sources.split("fn claim_source_retry(", 1)[1].split("\n}", 1)[0]
    retry_inner = sources.split("fn retry_source_ingestion_inner(", 1)[1].split("\n}", 1)[0]
    retry_public = sources.split("pub async fn retry_source_ingestion(", 1)[1].split("\n}", 1)[0]
    if "reset_source_for_reingestion" not in retry_claim:
        errors.append("retry claim must own the canonical reset")
    claim = retry_inner.find("claim_source_retry")
    projection = retry_inner.find("if !old_embedding_ids.is_empty()")
    if claim < 0 or projection < 0 or claim > projection:
        errors.append("retry source: canonical claim must precede projection removal")
    if "retry_source_ingestion_inner" not in retry_public:
        errors.append("retry source command must delegate to the observed retry owner")
    return errors


def check_pre_stream_chat_terminal_wiring(chat: str) -> list[str]:
    errors = []
    required_error_codes = [
        "semantic_memory_feature_disabled",
        "semantic_memory_projection_required",
        "semantic_memory_timeout",
        "semantic_memory_no_candidates",
        "semantic_memory_projection_failed",
        "semantic_memory_build_feature_missing",
    ]
    if chat.count("persist_pre_stream_chat_error(") < len(required_error_codes) + 1:
        errors.append("every strict semantic pre-stream error must use the terminal ledger owner")
    for code in required_error_codes:
        if f'"{code}"' not in chat:
            errors.append(f"missing typed pre-stream terminal error code: {code}")
    return errors


class NativePublicationWiringTests(unittest.TestCase):
    def setUp(self):
        self.state = (ROOT / "src-tauri/src/state.rs").read_text()
        self.sources = (ROOT / "src-tauri/src/commands/sources/mod.rs").read_text()
        self.chat = (ROOT / "src-tauri/src/commands/chat/mod.rs").read_text()

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

    def test_rejects_retry_without_canonical_claim(self):
        changed = self.sources.replace("db.reset_source_for_reingestion",
            "db.unchecked_retry_reset", 1)
        self.assertIn("retry claim must own the canonical reset", check_wiring(self.state, changed))

    def test_strict_semantic_pre_stream_errors_are_terminal(self):
        self.assertEqual(check_pre_stream_chat_terminal_wiring(self.chat), [])

    def test_rejects_missing_strict_semantic_terminal_owner_call(self):
        changed = self.chat.replace("persist_pre_stream_chat_error(", "missing_terminal_owner(", 2)
        self.assertTrue(check_pre_stream_chat_terminal_wiring(changed))
