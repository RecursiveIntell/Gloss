"""Execute the driver's actual readiness JavaScript against a bounded DOM fixture.

This checks input admission, not real WebKit rendering or native application UI.
"""
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock

SPEC = importlib.util.spec_from_file_location("readiness_driver", Path(__file__).resolve().parents[1] / "live_desktop_smoke.py")
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)


class ClickReadinessTests(unittest.TestCase):
    def fixture(self, inert_polls):
        ui = object.__new__(driver.WebDriver)
        ui.trace = []
        count = [0]
        def execute(script, args):
            count[0] += 1
            payload = {"script": script, "args": args, "inert": count[0] <= inert_polls}
            program = r"""
const input = JSON.parse(require('fs').readFileSync(0, 'utf8'));
global.innerWidth = 1000; global.innerHeight = 800;
const button = {id:'settings', disabled:false, textContent:'Settings', tagName:'BUTTON', title:'',
 getClientRects:()=>[{}], getBoundingClientRect:()=>({left:0,right:100,top:0,bottom:30,x:0,y:0,width:100,height:30}),
 closest:()=>input.inert ? {} : null, getAttribute:()=>null, contains:()=>false};
global.document = {querySelectorAll:()=>[button], elementFromPoint:()=>button};
process.stdout.write(JSON.stringify(new Function(input.script)(...input.args)));
"""
            result = subprocess.run(["node", "-e", program], input=json.dumps(payload), text=True, capture_output=True, check=True, timeout=5)
            return json.loads(result.stdout)
        def wait(condition, **_kwargs):
            for _ in range(6):
                result = condition()
                if result:
                    return result
            raise RuntimeError("fixture readiness deadline")
        ui.execute = execute
        ui.wait = wait
        ui.click_ref = Mock()
        return ui, count

    def test_visible_inert_control_is_not_clicked_until_released_and_stable(self):
        ui, count = self.fixture(2)
        ui.click_when_unobstructed("button")
        self.assertEqual(count[0], 4)
        ui.click_ref.assert_called_once()

    def test_persistent_inert_control_times_out_without_any_click(self):
        ui, _ = self.fixture(99)
        with self.assertRaisesRegex(RuntimeError, "deadline"):
            ui.click_when_unobstructed("button")
        ui.click_ref.assert_not_called()


if __name__ == "__main__":
    unittest.main()
