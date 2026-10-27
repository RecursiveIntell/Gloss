"""Native navigation/identity contracts; no synthetic runtime acceptance."""
import ast
import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import live_desktop_smoke as driver
from live_desktop_smoke import IntegratedWorkflow


READY = {'ready': True, 'rect': {'x': 10, 'y': 20, 'width': 100, 'height': 30}}


def stale_error(path='/session/fixture/execute/sync'):
    return driver.WebDriverHttpError('POST', path, 404,
                                     b'{"value":{"error":"stale element reference"}}')


class NavigationTransportFixture:
    """Command fixture only; does not certify native app acceptance."""
    def __init__(self):
        self.commands = []
        self.controls = []
        self.current = {driver.ELEMENT_KEY: 'current'}
        self.readiness = []
        self.last_readiness = READY
        self.at_end = False
        self.acknowledge_click = True
        self.click_error = None
        self.clock = 0.0

    def request(self, method, path, payload):
        self.commands.append((method, path, payload))
        if path.endswith('/execute/sync'):
            script = payload['script']
            if 'const region=document.querySelector' in script:
                return {'at_end': self.at_end, 'id': 'latest' if self.at_end else None}
            if "const nodes=Array.from(document.querySelectorAll(arguments[0])).filter" in script:
                if self.controls:
                    self.current = self.controls.pop(0)
                return self.current
            if 'const button=arguments[0], selector=arguments[1]' in script:
                if self.readiness:
                    self.last_readiness = self.readiness.pop(0)
                if isinstance(self.last_readiness, Exception):
                    raise self.last_readiness
                return copy.deepcopy(self.last_readiness)
            raise AssertionError('Unexpected or mutating JavaScript')
        if path.endswith('/click'):
            if self.click_error:
                raise self.click_error
            self.at_end = self.acknowledge_click
            return None
        raise AssertionError(f'Unexpected native command: {path}')

    def sleep(self, delay):
        self.clock += delay

    def mutations(self):
        return [row for row in self.commands if not row[1].endswith('/execute/sync')]


class ChatNavigationTests(unittest.TestCase):
    def workflow(self):
        workflow = object.__new__(IntegratedWorkflow)
        workflow.ui = Mock()
        workflow.ui.trace = []
        workflow.ui.execute.side_effect = lambda *_args: copy.deepcopy(READY)
        return workflow

    def test_atomic_snapshot_gates_identity_on_list_end_and_reads_same_bubble(self):
        workflow = self.workflow()
        workflow.answer_snapshot()
        script, args = workflow.ui.execute.call_args.args
        program = r'''
const fs=require('fs'); const input=JSON.parse(fs.readFileSync(0,'utf8'));
const bubble={querySelector:()=>({innerText:'CORRECT_ANSWER'}),dataset:{chatMessageId:'new',chatMessageRole:'assistant'}};
const button={getAttribute:()=> 'evidence-new', closest:()=>bubble};
const region={dataset:{chatAtBottom:'true',chatLatestMessageId:'new'},querySelectorAll:()=>[button]};
global.document={querySelector:s=>s.includes('Chat messages')?region:null};
const run=()=>new Function(input.script)(...input.args);
const atEnd=run(); region.dataset.chatLatestMessageId='pending-user'; const pending=run();
region.dataset.chatAtBottom='false'; const away=run();
process.stdout.write(JSON.stringify({atEnd,away,pending}));
'''
        result = subprocess.run(['node', '-e', program], input=json.dumps({'script': script, 'args': args}),
                                capture_output=True, text=True, check=True)
        observed = json.loads(result.stdout)
        self.assertEqual(observed['atEnd']['id'], 'evidence-new')
        self.assertEqual(observed['atEnd']['text'], 'CORRECT_ANSWER')
        self.assertTrue(observed['atEnd']['latest'])
        self.assertFalse(observed['pending']['latest'])
        self.assertIsNone(observed['away']['id'])
        self.assertEqual(observed['away']['text'], '')
        self.assertNotIn('.click(', script)
        self.assertNotIn('scrollTo', script)

    def test_jump_click_is_once_then_read_only_acknowledgement(self):
        workflow = self.workflow()
        states = iter([{'at_end': False}, {'at_end': False}, {'at_end': False},
                       {'at_end': False}, {'at_end': True}])
        workflow.answer_snapshot = Mock(side_effect=lambda: next(states))
        workflow.ui.find_visible.return_value = 'actual-control'
        def wait(condition, **_kwargs):
            for _ in range(3):
                value = condition()
                if value: return value
            raise RuntimeError('navigation not acknowledged')
        workflow.ui.wait.side_effect = wait
        workflow.jump_to_latest()
        workflow.ui.click_ref.assert_called_once_with('actual-control')

    def test_automatic_end_acknowledgement_does_not_wait_for_disappeared_control(self):
        workflow = self.workflow()
        workflow.answer_snapshot = Mock(side_effect=[{'at_end': False}, {'at_end': True}, {'at_end': True}])
        workflow.ui.wait.side_effect = lambda condition, **kwargs: condition()
        workflow.jump_to_latest()
        workflow.ui.click_ref.assert_not_called()

    def test_click_error_is_not_retried_or_hidden(self):
        workflow = self.workflow()
        workflow.answer_snapshot = Mock(return_value={'at_end': False})
        workflow.ui.find_visible.return_value = 'actual-control'
        def wait(condition, **_kwargs):
            for _ in range(3):
                value = condition()
                if value: return value
            raise AssertionError('fixture readiness deadline')
        workflow.ui.wait.side_effect = wait
        workflow.ui.click_ref.side_effect = RuntimeError('ambiguous click failure')
        with self.assertRaisesRegex(RuntimeError, 'ambiguous'):
            workflow.jump_to_latest()
        workflow.ui.click_ref.assert_called_once()

    def test_wait_does_not_accept_old_row_offscreen_or_pending_hydration(self):
        workflow = self.workflow()
        states = iter([{'at_end': True, 'latest': True, 'id': 'old', 'text': 'old', 'streaming': False},
                       {'at_end': False, 'latest': False, 'id': None, 'text': '', 'streaming': False},
                       {'at_end': True, 'latest': False, 'id': 'newly-mounted-old-answer', 'text': 'stale', 'streaming': False},
                       {'at_end': True, 'latest': True, 'id': 'new', 'text': 'saved', 'streaming': True},
                       {'at_end': True, 'latest': True, 'id': 'new', 'text': 'saved', 'streaming': False}])
        workflow.answer_snapshot = Mock(side_effect=lambda: next(states))
        def wait(condition, **kwargs):
            for _ in range(5):
                value = condition()
                if value: return value
            raise AssertionError('no complete new answer')
        workflow.ui.wait.side_effect = wait
        self.assertEqual(workflow.wait_for_answer('old', 'fixture')['text'], 'saved')
        workflow.ui.click_ref.assert_not_called()


class NavigationOrchestrationTests(unittest.TestCase):
    def setUp(self):
        self.fixture = NavigationTransportFixture()
        self.workflow = object.__new__(IntegratedWorkflow)
        self.workflow.ui = driver.WebDriver(1)
        self.workflow.ui.session = 'fixture'
        self.workflow.ui.request = self.fixture.request
        for item in (patch.object(driver.time, 'monotonic', side_effect=lambda: self.fixture.clock),
                     patch.object(driver.time, 'sleep', side_effect=self.fixture.sleep)):
            item.start()
            self.addCleanup(item.stop)

    def test_stale_read_only_probe_reacquires_live_control_before_one_click(self):
        old = {driver.ELEMENT_KEY: 'old'}
        new = {driver.ELEMENT_KEY: 'new'}
        self.fixture.controls = [old, new, new]
        self.fixture.readiness = [stale_error(), READY, READY]
        self.assertTrue(self.workflow.jump_to_latest()['at_end'])
        self.assertEqual([row[1] for row in self.fixture.mutations()],
                         ['/session/fixture/element/new/click'])
        self.assertEqual(sum(row.get('stale_before_click', False)
                             for row in self.workflow.ui.trace), 1)
        self.assertEqual(self.fixture.clock, 0.4)

    def test_replaced_control_must_be_stable_before_it_can_receive_one_click(self):
        self.fixture.controls = [{driver.ELEMENT_KEY: name} for name in ('old', 'new', 'new')]
        self.workflow.jump_to_latest()
        self.assertEqual([row[1] for row in self.fixture.mutations()],
                         ['/session/fixture/element/new/click'])
        self.assertEqual(self.fixture.clock, 0.4)

    def test_moving_control_must_be_stable_before_click(self):
        moved = {'ready': True, 'rect': {**READY['rect'], 'x': 20}}
        self.fixture.readiness = [READY, moved, moved]
        self.workflow.jump_to_latest()
        self.assertEqual(len(self.fixture.mutations()), 1)
        self.assertEqual(self.fixture.clock, 0.4)

    def test_persistent_stale_probe_exhausts_original_deadline_without_click(self):
        self.fixture.last_readiness = stale_error()
        with self.assertRaisesRegex(RuntimeError, 'navigation control timed out after 30s'):
            self.workflow.jump_to_latest()
        self.assertEqual(self.fixture.mutations(), [])
        self.assertGreaterEqual(self.fixture.clock, 30)
        self.assertLessEqual(self.fixture.clock, 30.2)

    def test_non_stale_read_only_errors_fail_without_retry_or_click(self):
        for error in (RuntimeError('ambiguous read failure'),
                      driver.WebDriverHttpError('POST', '/session/fixture/execute/sync', 404,
                                                b'{"value":{"error":"no such element"}}')):
            with self.subTest(error=type(error).__name__):
                self.fixture.commands.clear()
                self.fixture.last_readiness = error
                with self.assertRaises(type(error)) as raised:
                    self.workflow.jump_to_latest()
                self.assertIs(raised.exception, error)
                self.assertEqual(self.fixture.mutations(), [])
                self.assertEqual(self.fixture.clock, 0)

    def test_stale_or_ambiguous_click_response_remains_fatal_after_one_dispatch(self):
        for error in (stale_error('/session/fixture/element/current/click'),
                      RuntimeError('native click may have acted')):
            with self.subTest(error=type(error).__name__):
                self.fixture.commands.clear()
                self.fixture.click_error = error
                with self.assertRaises(type(error)) as raised:
                    self.workflow.jump_to_latest()
                self.assertIs(raised.exception, error)
                self.assertEqual([row[1] for row in self.fixture.mutations()],
                                 ['/session/fixture/element/current/click'])
                self.assertFalse(any(row.get('stale_before_click')
                                     for row in self.workflow.ui.trace))

    def test_missing_end_acknowledgement_still_fails_without_a_second_click(self):
        self.fixture.acknowledge_click = False
        with self.assertRaisesRegex(RuntimeError, 'navigation acknowledged timed out after 30s'):
            self.workflow.jump_to_latest()
        self.assertEqual([row[1] for row in self.fixture.mutations()],
                         ['/session/fixture/element/current/click'])


class NavigationReadinessJavaScriptTests(unittest.TestCase):
    def test_actual_probe_checks_live_identity_visibility_and_native_click_admission(self):
        tree = ast.parse(Path(driver.__file__).read_text())
        script = next(node.value for node in ast.walk(tree) if isinstance(node, ast.Constant)
                      and isinstance(node.value, str)
                      and 'const button=arguments[0], selector=arguments[1]' in node.value)
        program = r'''
const input=JSON.parse(require('fs').readFileSync(0,'utf8'));
global.innerWidth=600; global.innerHeight=400;
let inert=false, visible=true, owned=true, duplicate=false, current=true, replaced=false;
const button={isConnected:true,disabled:false,
 getClientRects:()=>visible?[{}]:[],
 getBoundingClientRect:()=>({left:10,right:110,top:20,bottom:50,x:10,y:20,width:100,height:30}),
 getAttribute:()=>null,closest:()=>inert?{}:null,contains:()=>false};
const other={getClientRects:()=>[{}]};
global.document={querySelectorAll:()=>!current?[]:duplicate?[button,other]:replaced?[other]:[button],
 elementFromPoint:()=>owned?button:{}};
const run=()=>new Function(input.script)(button,'button[aria-label="Jump to latest"]');
const ready=run(); replaced=true; const stale=run(); replaced=false;
button.isConnected=false; const disconnected=run(); button.isConnected=true;
visible=false; const hidden=run(); visible=true; inert=true; const inertState=run(); inert=false;
owned=false; const obstructed=run(); owned=true; button.disabled=true; const disabled=run();
button.disabled=false; duplicate=true; const ambiguous=run();
process.stdout.write(JSON.stringify({ready,stale,disconnected,hidden,inertState,obstructed,disabled,ambiguous}));
'''
        result = subprocess.run(['node', '-e', program], input=json.dumps({'script': script}),
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        observed = json.loads(result.stdout)
        self.assertTrue(observed.pop('ready')['ready'])
        self.assertTrue(all(not state['ready'] for state in observed.values()))
        self.assertNotIn('.click(', script)
        self.assertNotIn('scrollTo', script)


if __name__ == '__main__':
    unittest.main()
