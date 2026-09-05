import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('vendor_gate', Path(__file__).parents[1] / 'validate_vendor_closure.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class VendorClosureTests(unittest.TestCase):
    def fixture(self, content):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        package = root / 'src-tauri/vendor/example'
        package.mkdir(parents=True)
        (package / 'Cargo.toml').write_text(content)
        return root, package

    def test_detects_malformed_dependency_table(self):
        root, _ = self.fixture('[dependencies]\nx = "{ path = "../x" }')
        self.assertIn('invalid manifest', gate.validate(root)[0])

    def test_optional_target_dependency_must_exist(self):
        root, _ = self.fixture('[target.unix.dependencies]\nx = { path = "../x", optional = true }')
        self.assertIn('x missing', gate.validate(root)[0])

    def test_detects_dangling_link_and_missing_workspace_member(self):
        root, package = self.fixture('[workspace]\nmembers = ["core"]')
        (package.parent / 'broken').symlink_to('missing')
        errors = gate.validate(root)
        self.assertTrue(any('dangling symlink' in error for error in errors))
        self.assertTrue(any('missing workspace member' in error for error in errors))

    def test_accepts_real_local_dependency_without_treating_lib_path_as_dependency(self):
        root, package = self.fixture('[dependencies]\nx = { path = "../x" }\n[lib]\npath = "src/lib.rs"')
        (package.parent / 'x').mkdir()
        (package.parent / 'x/Cargo.toml').write_text('[package]\nname="x"\nversion="0.1.0"')
        self.assertEqual(gate.validate(root), [])
