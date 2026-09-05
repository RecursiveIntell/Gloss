"""Caller ownership checks supplement real semaphore/HTTP/native tests.

These source guards do not certify the full Tauri runtime or adapter profile.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


def body(source: str, name: str) -> str:
    return source.split(f"fn {name}(", 1)[1].split("\n}", 1)[0]


def check_guards(state: str, sources: str, chat: str, settings: str) -> list[str]:
    failures = []
    for name, call in [
        ("maybe_auto_project_semantic_memory", "semantic_memory_adapter::reindex_source("),
        ("semantic_memory_reindex_source", "semantic_memory_adapter::reindex_source("),
        ("semantic_memory_backfill_notebook", "semantic_memory_adapter::reindex_source_with_options("),
        ("semantic_memory_rebuild_vector_artifacts", "semantic_memory_adapter::rebuild_vector_artifacts("),
        ("compare_memory_backends", "compare_memory_backends_for_notebook("),
    ]:
        target = body(sources, name)
        guard = target.find("native_gates::acquire")
        if guard < 0 or guard > target.find(call):
            failures.append(name)
    preview = chat.split("let preview_result = tokio::time::timeout(", 1)[1].split("match preview_result", 1)[0]
    if not (0 <= preview.find("async {") < preview.find("native_gates::acquire") < preview.find("semantic_memory_adapter::search_preview(")):
        failures.append("chat preview owns guard within cancellable future")
    if not (preview.find("native_gates::acquire") < preview.find("semantic_memory_runtime_config_from_state") < preview.find("semantic_memory_adapter::search_preview(")):
        failures.append("chat checks configuration after waiting")
    auto_projection = body(sources, "maybe_auto_project_semantic_memory")
    if auto_projection.rfind("read_runtime_config()?") < auto_projection.find("native_gates::acquire_blocking"):
        failures.append("projection reads configuration after waiting")
    rebuild = body(sources, "native_dense_rebuild")
    if not (0 <= rebuild.find("spawn_blocking(move ||") < rebuild.find("native_gates::acquire_blocking") < rebuild.find("ensure_embedder_guarded")):
        failures.append("native rebuild worker owns inference permits")
    if "let guard = self.try_native_inference()?" not in body(state,"ensure_embedder"):
        failures.append("native initialization")
    diagnostics = body(settings,"run_embedding_diagnostics")
    if "spawn_blocking" in diagnostics:
        failures.append("diagnostic probe must not detach from its permits")
    diagnostic = diagnostics.split('Ok(config) if config.provider == "ollama" => {',1)[1]
    if diagnostic.find("try_native_inference()") < 0 or diagnostic.find("try_native_inference()") > diagnostic.find("ollama_embed_sync"):
        failures.append("semantic diagnostic probe")
    return failures


def check_diagnostics(settings: str) -> list[str]:
    target = body(settings, "run_embedding_diagnostics")
    failures = []
    if "ensure_embedder" in target or "EmbeddingService::from_config" in target:
        failures.append("diagnostics must not initialize native state")
    if "service.configured_identity.as_ref() != Some(config)" not in target:
        failures.append("diagnostics must reject stale cached services")
    if "normalized_provider" in target or "provider: configured_provider.clone()" not in target:
        failures.append("diagnostics must preserve configured provider")
    if "probe_ollama_embedding_dimension" in target or "ollama_embed_sync" not in target:
        failures.append("semantic success requires actual embedding")
    if "semantic_memory_provider.probe_ok = Some(semantic_probe.is_ok())" not in target:
        failures.append("semantic success must reflect its independent probe")
    return failures


class EmbeddingGateWiring(unittest.TestCase):
    def setUp(self):
        self.state = (ROOT / "src-tauri/src/state.rs").read_text()
        self.sources = (ROOT / "src-tauri/src/commands/sources/mod.rs").read_text()
        self.chat = (ROOT / "src-tauri/src/commands/chat/mod.rs").read_text()
        self.settings = (ROOT / "src-tauri/src/commands/settings.rs").read_text()

    def test_current_paths_use_shared_inference_owner(self):
        self.assertEqual(check_guards(self.state,self.sources,self.chat,self.settings), [])

    def test_removed_projection_guard_is_detected(self):
        changed = self.sources.replace("native_gates::acquire_blocking", "native_gates::unprotected")
        self.assertIn("maybe_auto_project_semantic_memory",check_guards(self.state,changed,self.chat,self.settings))

    def test_detached_chat_guard_is_detected(self):
        changed = self.chat.replace("native_gates::acquire", "native_gates::unprotected")
        self.assertIn("chat preview owns guard within cancellable future",check_guards(self.state,self.sources,changed,self.settings))

    def test_removed_configuration_recheck_is_detected(self):
        changed = self.chat.replace("semantic_memory_runtime_config_from_state", "old_config_snapshot")
        self.assertIn("chat checks configuration after waiting",check_guards(self.state,self.sources,changed,self.settings))

    def test_rebuild_guard_outside_worker_is_detected(self):
        changed = self.sources.replace("spawn_blocking(move ||", "detached_worker(move ||", 1)
        self.assertIn("native rebuild worker owns inference permits",check_guards(self.state,changed,self.chat,self.settings))

    def test_diagnostics_remain_independent_and_configuration_bound(self):
        self.assertEqual(check_diagnostics(self.settings), [])

    def test_diagnostics_native_initialization_is_detected(self):
        changed = self.settings.replace("let cache_dir = state.data_dir", "state.ensure_embedder(None)?; let cache_dir = state.data_dir")
        self.assertIn("diagnostics must not initialize native state", check_diagnostics(changed))

    def test_diagnostics_stale_service_probe_is_detected(self):
        changed = self.settings.replace("service.configured_identity.as_ref() != Some(config)", "false")
        self.assertIn("diagnostics must reject stale cached services", check_diagnostics(changed))

    def test_diagnostics_metadata_only_success_is_detected(self):
        changed = self.settings.replace("ollama_embed_sync", "probe_ollama_embedding_dimension")
        self.assertIn("semantic success requires actual embedding", check_diagnostics(changed))


if __name__ == "__main__":
    unittest.main()
