"""WebDriver command and geometry contracts; not a live WebKit observation."""
from __future__ import annotations

import ast
import copy
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "live_desktop_smoke.py"
SPEC = importlib.util.spec_from_file_location("select_driver", SCRIPT)
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)
SELECT = {driver.ELEMENT_KEY: "select-id"}
OPTION = {driver.ELEMENT_KEY: "option-id"}
READY = {"same_select": True, "same_option": True, "focused": True, "enabled": True,
         "value_unchanged": True, "in_view": True, "select_hit": True, "top_hit_owned": True,
         "rect": {"x": 10, "y": 20, "width": 100, "height": 30},
         "viewport": {"left": 0, "top": 0, "width": 600, "height": 400}}


class SelectTransportFixture:
    def __init__(self):
        self.commands = []
        self.eligible = True
        self.states = [copy.deepcopy(READY)]
        self.last_state = copy.deepcopy(READY)
        self.preparation_error = False
        self.click_error = False
        self.commit_value = True
        self.selected = "before"
        self.clock = 0.0

    def request(self, method, path, payload):
        self.commands.append((method, path, payload))
        if path.endswith("/execute/sync"):
            script = payload["script"]
            if "original_value:s.value" in script:
                return {"select": SELECT, "option": OPTION, "original_value": self.selected} if self.eligible else None
            if "const s=arguments[1], o=arguments[2]" in script:
                if self.states:
                    self.last_state = self.states.pop(0)
                return copy.deepcopy(self.last_state)
            if script == "return document.querySelector(arguments[0])?.value===arguments[1]":
                return self.selected == payload["args"][1]
            raise AssertionError("Unexpected or mutating JavaScript")
        if path.endswith("/select-id/value"):
            assert payload == {"text": "\ue008\ue008"}
            if self.preparation_error:
                raise RuntimeError("focus preparation failed")
            return None
        if path.endswith("/option-id/click"):
            if self.click_error:
                raise RuntimeError("option click may have acted")
            if self.commit_value:
                self.selected = "target"
            return None
        raise AssertionError(f"Unexpected native command: {path}")

    def sleep(self, delay):
        self.clock += delay

    def mutations(self):
        return [row for row in self.commands if not row[1].endswith("/execute/sync")]


class SelectOrchestrationTests(unittest.TestCase):
    def setUp(self):
        self.fixture = SelectTransportFixture()
        self.ui = driver.WebDriver(1)
        self.ui.session = "fixture"
        self.ui.request = self.fixture.request
        for item in (patch.object(driver.time, "monotonic", side_effect=lambda: self.fixture.clock),
                     patch.object(driver.time, "sleep", side_effect=self.fixture.sleep)):
            item.start()
            self.addCleanup(item.stop)

    def test_focus_once_waits_for_scroll_and_hit_testing_then_clicks_once(self):
        self.fixture.states = [{**READY, "in_view": False}, {**READY, "select_hit": False}, READY]
        self.ui.select("select.fixture", "target")
        self.assertEqual([row[1] for row in self.fixture.mutations()],
                         ["/session/fixture/element/select-id/value", "/session/fixture/element/option-id/click"])
        self.assertEqual(self.fixture.clock, 0.4)
        self.assertEqual(self.fixture.selected, "target")
        self.assertEqual(self.ui.trace[-1]["target_geometry"], READY)

    def test_missing_or_disabled_target_never_receives_preparation_or_click(self):
        self.fixture.eligible = False
        with self.assertRaisesRegex(RuntimeError, "selectable target timed out"):
            self.ui.select("select.fixture", "target")
        self.assertEqual(self.fixture.mutations(), [])

    def test_changed_select_or_option_fails_before_the_option_click(self):
        for flag in ("same_select", "same_option"):
            with self.subTest(flag=flag):
                self.fixture.commands.clear()
                self.fixture.states = [{**READY, flag: False}]
                with self.assertRaisesRegex(RuntimeError, "target changed"):
                    self.ui.select("select.fixture", "target")
                self.assertEqual(len(self.fixture.mutations()), 1)

    def test_preparation_cannot_change_the_selected_value(self):
        self.fixture.states = [{**READY, "value_unchanged": False}]
        with self.assertRaisesRegex(RuntimeError, "value changed"):
            self.ui.select("select.fixture", "target")
        self.assertEqual(len(self.fixture.mutations()), 1)

    def test_lost_focus_disabled_or_clipped_target_fails_at_the_readiness_deadline(self):
        for flag in ("focused", "enabled", "in_view", "select_hit", "top_hit_owned"):
            with self.subTest(flag=flag):
                self.fixture.commands.clear()
                self.fixture.clock = 0
                self.fixture.states = [{**READY, flag: False}]
                with self.assertRaisesRegex(RuntimeError, "interactable target timed out after 5s"):
                    self.ui.select("select.fixture", "target")
                self.assertEqual(len(self.fixture.mutations()), 1)
                self.assertLessEqual(self.fixture.clock, 5.2)
                self.assertFalse(self.ui.trace[-1]["target_geometry"][flag])

    def test_failed_preparation_is_not_ignored_or_replayed(self):
        self.fixture.preparation_error = True
        with self.assertRaisesRegex(RuntimeError, "focus preparation failed"):
            self.ui.select("select.fixture", "target")
        self.assertEqual(len(self.fixture.mutations()), 1)

    def test_failed_option_click_is_not_replayed(self):
        self.fixture.click_error = True
        with self.assertRaisesRegex(RuntimeError, "may have acted"):
            self.ui.select("select.fixture", "target")
        self.assertEqual(len(self.fixture.mutations()), 2)
        self.assertEqual(sum(row[1].endswith("/click") for row in self.fixture.commands), 1)

    def test_exact_post_click_value_is_still_required_without_another_click(self):
        self.fixture.commit_value = False
        with self.assertRaisesRegex(RuntimeError, "selected target timed out"):
            self.ui.select("select.fixture", "target")
        self.assertEqual(len(self.fixture.mutations()), 2)
        self.assertEqual(self.fixture.selected, "before")


class SelectGeometryJavaScriptTests(unittest.TestCase):
    def test_actual_readiness_script_checks_nested_clipping_identity_and_visual_viewport(self):
        tree = ast.parse(SCRIPT.read_text())
        script = next(node.value for node in ast.walk(tree) if isinstance(node, ast.Constant)
                      and isinstance(node.value, str) and "const s=arguments[1], o=arguments[2]" in node.value)
        node = shutil.which("node")
        self.assertIsNotNone(node, "Node is required to execute the read-only geometry contract")
        fixture = r'''
const script = JSON.parse(process.argv[1]);
function observe(c) {
  const rect=Object.freeze(c.rect || {x:10,y:20,left:10,top:20,right:110,bottom:50,width:100,height:30});
  const option=Object.freeze({isConnected:!c.detachedOption,value:'target',matches:()=>!!c.disabledOption});
  const other=Object.freeze({tagName:'UNRELATED',value:'unrelated-secret-input'});
  const select=Object.freeze({isConnected:!c.detachedSelect,value:c.changedValue?'changed':'before',
    options:Object.freeze(c.replacedOption?[]:[option]),matches:()=>!!c.disabledSelect,
    getClientRects:()=>c.noRect?[]:[rect],contains:e=>e===option,
    click:()=>{throw Error('raw click forbidden')},focus:()=>{throw Error('raw focus forbidden')}});
  let point=null;
  globalThis.window=Object.freeze({innerWidth:600,innerHeight:400,visualViewport:c.viewport || null});
  globalThis.document=Object.freeze({activeElement:c.lostFocus?other:select,
    querySelector:()=>c.replacedSelect?other:select,
    elementsFromPoint:(x,y)=>{point=[x,y];return c.clipped?[]:c.obscured?[other,select]:[select]}});
  const state=new Function('"use strict";'+script).apply(null,['select.fixture',select,option,'target','before']);
  return {state,point};
}
const cases={ready:{},offscreen:{rect:{x:10,y:500,left:10,top:500,right:110,bottom:530,width:100,height:30}},
 clipped:{clipped:true},obscured:{obscured:true},noRect:{noRect:true},disabled:{disabledOption:true},
 replacedSelect:{replacedSelect:true},replacedOption:{replacedOption:true},lostFocus:{lostFocus:true},
 changedValue:{changedValue:true},visualViewport:{viewport:{offsetLeft:40,offsetTop:25,width:60,height:100}}};
process.stdout.write(JSON.stringify(Object.fromEntries(Object.entries(cases).map(([k,v])=>[k,observe(v)]))));
'''
        result = subprocess.run([node, "-e", fixture, json.dumps(script)], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("unrelated-secret-input", result.stdout)
        rows = json.loads(result.stdout)
        self.assertTrue(rows["ready"]["state"]["select_hit"])
        self.assertEqual(rows["ready"]["point"], [60, 35])
        self.assertFalse(rows["offscreen"]["state"]["in_view"])
        self.assertIsNone(rows["offscreen"]["point"])
        self.assertFalse(rows["clipped"]["state"]["select_hit"])
        self.assertTrue(rows["clipped"]["state"]["in_view"])
        self.assertTrue(rows["obscured"]["state"]["select_hit"])
        self.assertFalse(rows["obscured"]["state"]["top_hit_owned"])
        self.assertIsNone(rows["noRect"]["state"]["rect"])
        for case, flag in (("disabled", "enabled"), ("replacedSelect", "same_select"),
                           ("replacedOption", "same_option"), ("lostFocus", "focused"),
                           ("changedValue", "value_unchanged")):
            self.assertFalse(rows[case]["state"][flag], case)
        self.assertEqual(rows["visualViewport"]["point"], [70, 37.5])


class SelectFailureDetailsTests(unittest.TestCase):
    def test_failure_owner_keeps_the_request_and_readiness_as_separate_details(self):
        tree = ast.parse(SCRIPT.read_text())
        assignments = next(node.body[:3] for node in ast.walk(tree)
                           if isinstance(node, ast.If) and isinstance(node.test, ast.Name)
                           and node.test.id == "driver" and node.body
                           and isinstance(node.body[0], ast.Assign)
                           and any(isinstance(target, ast.Name) and target.id == "last_request"
                                   for target in node.body[0].targets))
        code = compile(ast.fix_missing_locations(ast.Module(body=assignments, type_ignores=[])), str(SCRIPT), "exec")
        failed_request = {"method": "POST", "path": "/session/fixture/element/option-id/click", "error": "HTTP 400"}
        readiness = {"select_readiness": "select.fixture", "target_geometry": READY}
        for trace in ([failed_request, readiness], [readiness, failed_request], []):
            with self.subTest(trace=trace):
                ui = driver.WebDriver(1)
                ui.trace = trace
                scope = {"driver": ui, "details": {}, "json": json}
                exec(code, scope)
                if trace:
                    self.assertEqual(json.loads(scope["details"]["last_webdriver_request"]), failed_request)
                    self.assertEqual(scope["details"]["last_select_readiness"], readiness)
                else:
                    self.assertIsNone(scope["details"]["last_webdriver_request"])
                    self.assertIsNone(scope["details"]["last_select_readiness"])


if __name__ == "__main__":
    unittest.main()
