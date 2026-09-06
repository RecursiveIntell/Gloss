"""Native navigation/identity contracts; no synthetic runtime acceptance."""
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from live_desktop_smoke import IntegratedWorkflow


class ChatNavigationTests(unittest.TestCase):
    def workflow(self):
        workflow = object.__new__(IntegratedWorkflow)
        workflow.ui = Mock()
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
        states = iter([{'at_end': False}, {'at_end': False}, {'at_end': False}, {'at_end': True}])
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
        workflow.ui.wait.side_effect = lambda condition, **kwargs: condition()
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


if __name__ == '__main__':
    unittest.main()
