"""The independent edge acceptance has no production deployment authority."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from ci_test_map import ISOLATED_GATEWAY_WORKFLOWS


class FallbackWorkflowTests(unittest.TestCase):
    def test_registered_isolated_read_only_workflow(self):
        name = "tor-fallback-edge.yml"
        self.assertIn(name, ISOLATED_GATEWAY_WORKFLOWS)
        text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
        for required in (
            "  pull_request:", "  workflow_dispatch:", "contents: read",
            "persist-credentials: false", "python3 tests/edge/fallback_smoke.py",
        ):
            self.assertIn(required, text)
        for forbidden in (
            "  push:", "pull_request_target:", "secrets.", "secrets:",
            "environment: tor-staging", "contents: write", "continue-on-error:",
            "ssh ", "scp ", "sudo systemctl",
        ):
            self.assertNotIn(forbidden, text)
        for path in ("tests/edge/fallback_smoke.py", "tests/edge/fallback_browser.cjs"):
            self.assertTrue((ROOT / path).is_file())
