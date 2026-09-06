"""Driver synchronization contracts; real native acceptance remains separate."""
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from live_desktop_smoke import IntegratedWorkflow, WebDriver


class SourceActionReadinessTests(unittest.TestCase):
    def workflow(self):
        workflow = object.__new__(IntegratedWorkflow)
        workflow.ui = Mock()
        return workflow

    def test_delayed_virtual_row_mount_precedes_the_single_native_click(self):
        workflow = self.workflow()
        workflow.ui.execute.side_effect = [None, None, 'ready-button']
        def wait(condition, **kwargs):
            for _ in range(3):
                workflow.ui.click_ref.assert_not_called()
                result = condition()
                if result:
                    return result
            raise RuntimeError('source not ready')
        workflow.ui.wait.side_effect = wait
        workflow.click_source_button('Recovery fixture', 'button[title="Retry ingestion"]')
        self.assertEqual(workflow.ui.execute.call_count, 3)
        workflow.ui.click_ref.assert_called_once_with('ready-button')

    def test_missing_source_times_out_without_clicking_any_other_row(self):
        workflow = self.workflow()
        workflow.ui.execute.return_value = None
        def wait(condition, **kwargs):
            self.assertIsNone(condition())
            raise RuntimeError('visible source action timed out')
        workflow.ui.wait.side_effect = wait
        with self.assertRaisesRegex(RuntimeError, 'timed out'):
            workflow.click_source_button('Recovery fixture', 'button[title="Retry ingestion"]')
        workflow.ui.click_ref.assert_not_called()

    def test_native_click_failure_is_not_retried(self):
        workflow = self.workflow()
        workflow.ui.wait.return_value = 'button'
        workflow.ui.click_ref.side_effect = RuntimeError('intercepted click')
        with self.assertRaisesRegex(RuntimeError, 'intercepted'):
            workflow.click_source_button('Recovery fixture', 'button[title="Retry ingestion"]')
        workflow.ui.click_ref.assert_called_once_with('button')

    def test_retry_command_error_is_retained_before_alert_expires(self):
        for title in ['Retry Failed', 'Reindex Failed', 'Reindex Complete']:
            with self.subTest(title=title):
                workflow = self.workflow()
                workflow.ui.execute.return_value = [{'title': title, 'message': 'original immediate diagnostic'}]
                workflow.ui.wait.side_effect = lambda condition, **kwargs: condition()
                with self.assertRaisesRegex(RuntimeError, 'original immediate diagnostic'):
                    workflow.wait_source_retry('Recovery fixture', 'old embedding error')
                workflow.ui.click_ref.assert_not_called()

    def test_retry_wait_requires_the_requested_sources_new_terminal_state(self):
        workflow = self.workflow()
        workflow.ui.execute.return_value = []
        workflow.source_rows = Mock()
        workflow.ui.wait.side_effect = lambda condition, **kwargs: condition()
        for title, status, error, expected in [
            ('Recovery fixture', 'error', 'old embedding error', False),
            ('Recovery fixture', 'pending', None, False),
            ('different source', 'ready', None, False),
            ('Recovery fixture', 'ready', None, True),
            ('Recovery fixture', 'error', 'new embedding error', True),
        ]:
            workflow.source_rows.return_value = [{'title': title, 'status': status, 'error': error}]
            results = []
            workflow.ui.wait.side_effect = lambda condition, **kwargs: results.append(condition())
            workflow.wait_source_retry('Recovery fixture', 'old embedding error')
            self.assertEqual(results, [expected])
        workflow.ui.click_ref.assert_not_called()

    def test_readonly_query_rejects_absent_hidden_ambiguous_and_disabled_actions(self):
        workflow = self.workflow()
        workflow.ui.wait.side_effect = lambda condition, **kwargs: condition()
        workflow.click_source_button('Recovery fixture', 'button[title="Retry ingestion"]')
        script, args = workflow.ui.execute.call_args.args
        program = r'''
const fs=require('fs'); const input=JSON.parse(fs.readFileSync(0,'utf8'));
let visible=true, disabled=false, actionVisible=true, marker=true;
const button={id:'retry',get disabled(){return disabled},getClientRects:()=>actionVisible?[{}]:[]};
const row={querySelector:s=>s.includes('Reindex')?(marker?{}:null):button};
const label={title:'Recovery fixture',getClientRects:()=>visible?[{}]:[],parentElement:{parentElement:row}};
let labels=[]; global.document={querySelectorAll:()=>labels};
const run=()=>new Function(input.script)(...input.args);
const missing=run(); labels=[label]; const ready=run()?.id;
visible=false;const hidden=run();visible=true;
labels=[label,label];const ambiguous=run();labels=[label];
disabled=true;const blocked=run();disabled=false;
actionVisible=false;const hiddenAction=run();actionVisible=true;
marker=false;const wrongRow=run();
process.stdout.write(JSON.stringify({missing,ready,hidden,ambiguous,blocked,hiddenAction,wrongRow}));
'''
        result = subprocess.run(['node', '-e', program], input=json.dumps({'script': script, 'args': args}),
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout), {'missing': None, 'ready': 'retry', 'hidden': None,
                         'ambiguous': None, 'blocked': None, 'hiddenAction': None, 'wrongRow': None})
        for mutation in ['.click(', 'scrollTo', 'dispatchEvent', '__TAURI', '.value =']:
            self.assertNotIn(mutation, script)


class InspectorHitReadinessTests(unittest.TestCase):
    def driver(self):
        driver = WebDriver(0)
        driver.execute = Mock()
        driver.click_ref = Mock()
        return driver

    def test_transient_blocker_clears_before_one_native_click(self):
        driver = self.driver()
        driver.execute.side_effect = [{'ready': False, 'hit': {'label': 'Dismiss notification'}},
                                      {'ready': True, 'button': 'inspector', 'rect': {'x': 100}},
                                      {'ready': True, 'button': 'inspector', 'rect': {'x': 100}}]
        def wait(condition, **kwargs):
            self.assertIsNone(condition())
            self.assertIsNone(condition())
            driver.click_ref.assert_not_called()
            return condition()
        driver.wait = wait
        driver.click_when_unobstructed('button[aria-label="Open inspector"]')
        driver.click_ref.assert_called_once_with('inspector')
        self.assertEqual(driver.trace[0]['observation']['hit']['label'], 'Dismiss notification')

    def test_moving_or_replaced_target_requires_new_stable_observations(self):
        driver = self.driver()
        driver.execute.side_effect = [
            {'ready': True, 'button': 'old', 'rect': {'x': 100}},
            {'ready': True, 'button': 'old', 'rect': {'x': 105}},
            {'ready': True, 'button': 'new', 'rect': {'x': 105}},
            {'ready': False},
            {'ready': True, 'button': 'new', 'rect': {'x': 105}},
            {'ready': True, 'button': 'new', 'rect': {'x': 105}},
        ]
        def wait(condition, **kwargs):
            for _ in range(5):
                self.assertIsNone(condition())
                driver.click_ref.assert_not_called()
            return condition()
        driver.wait = wait
        driver.click_when_unobstructed('button')
        driver.click_ref.assert_called_once_with('new')

    def test_readiness_timeout_never_clicks_and_native_failure_is_not_replayed(self):
        driver = self.driver()
        driver.execute.return_value = {'ready': False}
        def timeout(condition, **kwargs):
            self.assertIsNone(condition())
            raise RuntimeError('readiness timeout')
        driver.wait = timeout
        with self.assertRaisesRegex(RuntimeError, 'readiness timeout'):
            driver.click_when_unobstructed('button')
        driver.click_ref.assert_not_called()
        driver.wait = lambda condition, **kwargs: 'inspector'
        driver.click_ref.side_effect = RuntimeError('native click failed')
        with self.assertRaisesRegex(RuntimeError, 'native click failed'):
            driver.click_when_unobstructed('button')
        driver.click_ref.assert_called_once_with('inspector')

    def test_actual_readonly_query_rejects_occlusion_and_clipping(self):
        driver = self.driver()
        driver.execute.return_value = {'ready': True, 'button': 'inspector'}
        driver.wait = lambda condition, **kwargs: (condition(), condition())[1]
        driver.click_when_unobstructed('button')
        script, args = driver.execute.call_args.args
        program = r'''
const fs=require('fs'),input=JSON.parse(fs.readFileSync(0,'utf8'));
global.innerWidth=1400;global.innerHeight=900;
let visible=true,disabled=false,rect={left:1348,right:1400,top:60,bottom:98,x:1348,y:60,width:52,height:38};
const child={tagName:'svg',getAttribute:()=>null};
const button={getClientRects:()=>visible?[{}]:[],getBoundingClientRect:()=>rect,
 get disabled(){return disabled},contains:e=>e===child,tagName:'BUTTON',getAttribute:()=>null};
const blocker={tagName:'BUTTON',getAttribute:()=> 'Dismiss notification'};
let nodes=[button],hit=blocker;
global.document={querySelectorAll:()=>nodes,elementFromPoint:()=>hit};
const run=()=>new Function(input.script)(...input.args).ready;
const occluded=run();hit=button;const clear=run();hit=child;const descendant=run();
disabled=true;const blocked=run();disabled=false;visible=false;const hidden=run();visible=true;
nodes=[button,button];const ambiguous=run();nodes=[button];
rect={left:1500,right:1552,top:60,bottom:98};const offscreen=run();
process.stdout.write(JSON.stringify({occluded,clear,descendant,blocked,hidden,ambiguous,offscreen}));
'''
        result = subprocess.run(['node', '-e', program], input=json.dumps({'script': script, 'args': args}),
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout), {'occluded': False, 'clear': True, 'descendant': True,
                         'blocked': False, 'hidden': False, 'ambiguous': False, 'offscreen': False})
        for mutation in ['.click(', 'scrollTo', 'dispatchEvent', '__TAURI', '.value =']:
            self.assertNotIn(mutation, script)


if __name__ == '__main__':
    unittest.main()
