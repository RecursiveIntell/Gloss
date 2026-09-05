"""Receipt identity must change when executed source changes, even without a commit."""

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path


spec = importlib.util.spec_from_file_location(
    "source_snapshot", Path(__file__).resolve().parents[1] / "source_snapshot.py"
)
snapshot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(snapshot)


class SourceSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        (self.root / "source.txt").write_text("before")
        (self.root / ".gitignore").write_text("receipts/\n")
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)

    def test_same_revision_with_changed_source_has_different_identity(self):
        before = snapshot.capture_source_identity(self.root)
        (self.root / "source.txt").write_text("after")
        after = snapshot.capture_source_identity(self.root)
        self.assertEqual(before["revision"], after["revision"])
        self.assertNotEqual(before["source_sha256"], after["source_sha256"])
        self.assertTrue(before["worktree_clean"])
        self.assertFalse(after["worktree_clean"])

    def test_new_files_are_bound_but_ignored_output_is_not_source(self):
        before = snapshot.capture_source_identity(self.root)
        (self.root / "receipts").mkdir()
        (self.root / "receipts" / "run.json").write_text("generated")
        self.assertEqual(before, snapshot.capture_source_identity(self.root))
        (self.root / "new.rs").write_text("fn main() {}")
        self.assertNotEqual(before["source_sha256"], snapshot.capture_source_identity(self.root)["source_sha256"])

    def test_deleted_tracked_source_changes_identity(self):
        before = snapshot.capture_source_identity(self.root)
        (self.root / "source.txt").unlink()
        self.assertNotEqual(before["source_sha256"], snapshot.capture_source_identity(self.root)["source_sha256"])

    def test_symlink_binds_target_without_reading_external_content(self):
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / "private.txt"
            target.write_text("first")
            (self.root / "link").symlink_to(target)
            before = snapshot.capture_source_identity(self.root)
            target.write_text("changed external content")
            self.assertEqual(before, snapshot.capture_source_identity(self.root))
            (self.root / "link").unlink()
            (self.root / "link").symlink_to(Path(outside) / "another.txt")
            self.assertNotEqual(before["source_sha256"], snapshot.capture_source_identity(self.root)["source_sha256"])


if __name__ == "__main__":
    unittest.main()
