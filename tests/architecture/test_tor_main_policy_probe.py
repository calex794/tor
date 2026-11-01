"""I-06 policy proof must run on trusted main pushes, never execute PR code with token."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class MainPolicyProbeTests(unittest.TestCase):
    def test_read_only_policy_check_on_main_push_only(self):
        flow = (ROOT / ".github/workflows/rules.yml").read_text(encoding="utf-8")
        self.assertIn("name: Read-only I-06 main ruleset proof on trusted push", flow)
        step = flow.split("      - name: Read-only I-06 main ruleset proof on trusted push", 1)[1]
        step = step.split("      - name:", 1)[0]
        self.assertIn("if: github.event_name == 'push'", step)
        self.assertIn("GITHUB_TOKEN: ${{ github.token }}", step)
        self.assertIn("--policy-only", step)
        self.assertIn("--sha \"${{ github.sha }}\"", step)
        self.assertNotIn("ssh ", step)
        self.assertNotIn("workflow_dispatch", step)


if __name__ == "__main__":
    unittest.main()
