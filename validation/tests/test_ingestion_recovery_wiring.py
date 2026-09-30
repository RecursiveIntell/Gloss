"""Static wiring complements the actual process-boundary DB integration fixture."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
class RecoveryWiring(unittest.TestCase):
    def test_recovery_runs_after_queue_load_before_worker_admission(self):
        text = (ROOT / 'src-tauri/src/lib.rs').read_text()
        self.assertLess(text.index('QueueManager::new(config)'), text.index('state.recover_interrupted_source_ingestions(&queue)'))
        self.assertLess(text.index('state.recover_interrupted_source_ingestions(&queue)'), text.index('app.manage(state)'))
        self.assertLess(text.index('state.recover_interrupted_source_ingestions(&queue)'), text.index('summary_job_loop(q,'))
    def test_inline_ingestion_panic_is_converted_to_terminal_failure(self):
        text = (ROOT / 'src-tauri/src/commands/sources/mod.rs').read_text()
        owner = text[text.index('fn run_ingestion_inner('):text.index('fn run_ingestion_inner(')+18000]
        self.assertIn('catch_unwind', owner)
        self.assertIn('ingestion_panicked:', owner)
