"""Native command-contract fixtures; these are not GTK/AppImage observations."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("folder_driver", Path(__file__).resolve().parents[1] / "live_desktop_smoke.py")
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)


class NativeChooserFixture:
    dialog = "2097961"

    def __init__(self):
        self.trace = []
        self.commands = []
        self.opened = False
        self.focus = "1"  # Xvfb PointerRoot before an explicit focus request.
        self.previous = {"97"}
        self.extra = set()
        self.appear_after = 0
        self.searches_after_open = 0
        self.returns = 0
        self.close_after = 1
        self.failed_action = None
        self.timeout_action = None
        self.lose_focus_after_location_shortcut = False
        self.clock = 0.0

    def find_visible(self, *_args):
        return False

    def click_text(self, label):
        assert label == "Folder"
        self.opened = True

    def execute(self, *_args):
        raise AssertionError("Native chooser path must not use JS/store/IPC injection")

    def sleep(self, seconds):
        self.clock += seconds

    def run(self, command, **kwargs):
        self.commands.append(command)
        assert 0 < kwargs["timeout"] <= 10
        assert kwargs["check"] is False
        action = command[1]
        if action == self.timeout_action:
            raise subprocess.TimeoutExpired(command, kwargs["timeout"], output=b"partial operation", stderr=b"native deadline")
        if action == self.failed_action:
            return subprocess.CompletedProcess(command, 7, "operation may have begun", "native command failed")
        if action == "search":
            assert "--sync" not in command
            windows = self.previous.copy()
            if self.opened:
                self.searches_after_open += 1
                if self.searches_after_open > self.appear_after and self.returns < self.close_after:
                    windows |= {self.dialog} | self.extra
            stdout = "\n".join(sorted(windows))
            return subprocess.CompletedProcess(command, 0 if windows else 1, stdout, "")
        if action == "getwindowname":
            assert command[2:] == [self.dialog]
            return subprocess.CompletedProcess(command, 0, "Select a Folder", "")
        if action == "windowfocus":
            assert command[2:] == ["--sync", self.dialog]
            self.focus = self.dialog
        elif action == "getwindowfocus":
            # Reproduce the no-WM failure for the old WM_CLASS-traversing call.
            if command[2:] != ["-f"]:
                return subprocess.CompletedProcess(command, 1, "", "getwindowfocus failed without WM_CLASS")
            return subprocess.CompletedProcess(command, 0, self.focus, "")
        elif action in ("key", "type"):
            assert self.focus == self.dialog
            if action == "key" and command[-1] == "Return":
                self.returns += 1
                if self.returns >= self.close_after:
                    self.focus = "1"
            if command[-1] == "ctrl+l" and self.lose_focus_after_location_shortcut:
                self.focus = "12345"
        else:
            raise AssertionError(f"Unexpected native operation: {command}")
        return subprocess.CompletedProcess(command, 0, "", "")


class NativeFolderChooserTests(unittest.TestCase):
    def setUp(self):
        self.native = NativeChooserFixture()
        self.folder = Path("/fixture/folder with spaces")
        self.workflow = driver.IntegratedWorkflow(self.native, Path("/fixture/gloss"), Path("/fixture"), {}, {})
        self.workflow.case_id = "folder_import_scope"
        self.patches = [patch.object(driver.shutil, "which", return_value="/fixture/xdotool"),
                        patch.object(driver.subprocess, "run", side_effect=self.native.run),
                        patch.object(driver.time, "monotonic", side_effect=lambda: self.native.clock),
                        patch.object(driver.time, "sleep", side_effect=self.native.sleep)]
        for item in self.patches:
            item.start()
            self.addCleanup(item.stop)

    def test_discovers_new_dialog_and_focuses_its_identity_without_a_window_manager(self):
        self.workflow.folder(self.folder)
        self.assertEqual(self.native.returns, 1)
        self.assertIn(["xdotool", "getwindowname", self.native.dialog], self.native.commands)
        self.assertIn(["xdotool", "windowfocus", "--sync", self.native.dialog], self.native.commands)
        self.assertIn(["xdotool", "type", "--clearmodifiers", "--delay", "1", str(self.folder)], self.native.commands)
        self.assertTrue(all(command[2:] == ["-f"] for command in self.native.commands if command[1] == "getwindowfocus"))
        self.assertFalse(any(command[1] == "windowactivate" for command in self.native.commands))

    def test_delayed_dialog_search_is_read_only_and_bounded(self):
        self.native.previous.clear()
        self.native.appear_after = 3
        self.workflow.folder(self.folder)
        self.assertGreaterEqual(self.native.clock, 0.6)
        self.assertEqual(sum(command[1] == "windowfocus" for command in self.native.commands), 1)

    def test_multiple_new_dialogs_fail_before_focus_or_keyboard_mutation(self):
        self.native.extra = {"2097962"}
        with self.assertRaisesRegex(RuntimeError, "unambiguous"):
            self.workflow.folder(self.folder)
        self.assertTrue(all(command[1] == "search" for command in self.native.commands))

    def test_absent_dialog_fails_at_deadline_without_mutating_a_previous_window(self):
        self.native.appear_after = 1000
        with self.assertRaisesRegex(RuntimeError, "within 10 seconds"):
            self.workflow.folder(self.folder)
        self.assertLessEqual(self.native.clock, 10.1)
        self.assertTrue(all(command[1] == "search" for command in self.native.commands))

    def test_focus_loss_aborts_before_typing_into_another_window(self):
        self.native.lose_focus_after_location_shortcut = True
        with self.assertRaisesRegex(RuntimeError, "retains keyboard focus"):
            self.workflow.folder(self.folder)
        self.assertFalse(any(command[1] == "type" for command in self.native.commands))
        self.assertEqual(sum(command[1] == "windowfocus" for command in self.native.commands), 1)

    def test_failed_mutation_is_traced_and_never_replayed(self):
        self.native.failed_action = "type"
        with self.assertRaises(subprocess.CalledProcessError):
            self.workflow.folder(self.folder)
        self.assertEqual(sum(command[1] == "type" for command in self.native.commands), 1)
        self.assertEqual(self.native.returns, 0)
        self.assertEqual(self.native.trace[-1]["exit_code"], 7)
        self.assertEqual(self.native.trace[-1]["stderr"], "native command failed")

    def test_focus_timeout_retains_partial_diagnostics_without_replaying_action(self):
        self.native.timeout_action = "windowfocus"
        with self.assertRaises(subprocess.TimeoutExpired):
            self.workflow.folder(self.folder)
        self.assertEqual(sum(command[1] == "windowfocus" for command in self.native.commands), 1)
        failed = self.native.trace[-1]
        self.assertTrue(failed["timed_out"])
        self.assertIsNone(failed["exit_code"])
        self.assertEqual(failed["stdout"], "partial operation")
        self.assertEqual(failed["stderr"], "native deadline")
        self.assertFalse(any(command[1] in ("key", "type") for command in self.native.commands))

    def test_native_search_error_is_not_treated_as_no_matching_window(self):
        self.native.failed_action = "search"
        with self.assertRaises(subprocess.CalledProcessError):
            self.workflow.folder(self.folder)
        self.assertFalse(self.native.opened)
        self.assertEqual(self.native.trace[-1]["stderr"], "native command failed")

    def test_same_dialog_can_navigate_then_confirm_and_must_disappear(self):
        self.native.close_after = 2
        self.workflow.folder(self.folder)
        self.assertEqual(self.native.returns, 2)
        self.assertEqual(self.native.focus, "1")

    def test_still_visible_dialog_fails_after_confirmation_without_more_key_actions(self):
        self.native.close_after = 100
        with self.assertRaisesRegex(RuntimeError, "did not close"):
            self.workflow.folder(self.folder)
        self.assertEqual(self.native.returns, 2)
        self.assertLessEqual(self.native.clock, 5.4)


if __name__ == "__main__":
    unittest.main()
