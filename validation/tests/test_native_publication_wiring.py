"""Source wiring guards supplement (not replace) actual DB/dense native tests."""
from pathlib import Path
import re
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


def terminal_owner_calls(chat: str) -> list[tuple[int, int, str]]:
    calls = []
    for match in re.finditer(r"persist_pre_stream_chat_error\s*\(", chat):
        depth, quoted, escaped = 1, False, False
        index = match.end()
        while index < len(chat) and depth:
            char = chat[index]
            if quoted:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    quoted = False
            elif char == '"':
                quoted = True
            elif char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            index += 1
        calls.append((match.start(), index, chat[match.end():index]))
    return calls


def check_pre_stream_chat_terminal_wiring(chat: str) -> list[str]:
    errors = []
    # Fixture-only calls are not production persistence ownership evidence.
    chat = chat.split("#[cfg(test)]", 1)[0]
    required_error_codes = [
        "semantic_memory_feature_disabled", "semantic_memory_projection_required",
        "semantic_memory_timeout", "semantic_memory_no_candidates",
        "semantic_memory_projection_failed", "semantic_memory_build_feature_missing",
        "semantic_memory_config_error",
    ]
    if not re.search(r"fn\s+persist_pre_stream_chat_error(?:<[^>]+>)?\s*\(", chat):
        errors.append("missing canonical pre-stream terminal ledger owner")
    calls = terminal_owner_calls(chat)
    for code in required_error_codes:
        if not any(f'"{code}"' in body for _, _, body in calls):
            errors.append(f"missing terminal owner call for typed pre-stream error: {code}")
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

    def test_each_typed_terminal_call_is_required_independent_of_total_count(self):
        codes = ["semantic_memory_feature_disabled", "semantic_memory_projection_required",
                 "semantic_memory_timeout", "semantic_memory_no_candidates",
                 "semantic_memory_projection_failed", "semantic_memory_build_feature_missing",
                 "semantic_memory_config_error"]
        for code in codes:
            # Keep surplus owner calls and the exact typed diagnostic elsewhere;
            # neither can substitute for this error's own persisted terminal.
            changed = self.chat.replace(f'"{code}"', f'"removed_{code}"')
            changed += '\n' + ('persist_pre_stream_chat_error("other");\n' * 20)
            changed += f'let diagnostic = "{code}";\n'
            self.assertIn(f"missing terminal owner call for typed pre-stream error: {code}",
                          check_pre_stream_chat_terminal_wiring(changed))

    def test_test_only_call_cannot_replace_any_production_terminal_call(self):
        production = self.chat.split("#[cfg(test)]", 1)[0]
        calls = terminal_owner_calls(production)
        codes = ["semantic_memory_feature_disabled", "semantic_memory_projection_required",
                 "semantic_memory_timeout", "semantic_memory_no_candidates",
                 "semantic_memory_projection_failed", "semantic_memory_build_feature_missing",
                 "semantic_memory_config_error"]
        for code in codes:
            start, _, _ = next(call for call in calls if f'"{code}"' in call[2])
            changed = self.chat[:start] + "missing_terminal_owner" + self.chat[start + len("persist_pre_stream_chat_error"):]
            changed += f'\n#[cfg(test)] fn extra_fixture() {{ persist_pre_stream_chat_error("{code}"); }}\n'
            self.assertIn(f"missing terminal owner call for typed pre-stream error: {code}",
                          check_pre_stream_chat_terminal_wiring(changed))
