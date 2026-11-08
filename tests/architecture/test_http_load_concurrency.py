"""Guard HTTP load against stale PR run backlogs without interrupting dispatch."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class HTTPLoadConcurrencyTests(unittest.TestCase):
    def test_selection_policy_edits_do_not_trigger_unrelated_http_load(self):
        text = (ROOT / ".github/workflows/tor-http-load.yml").read_text()
        event = text.split("\npermissions:", 1)[0]
        self.assertNotIn("      - tools/ci_test_map.py", event)
        for relevant_path in (
            "      - .github/workflows/tor-http-load.yml",
            "      - tests/load/**",
            "      - crates/search/src/repositories/catalog.rs",
            "      - migrations/202610300021_public_catalog_exact_projection.sql",
        ):
            self.assertIn(relevant_path, event)

    def test_pr_only_cancellation_keeps_manual_dispatch_isolated(self):
        text = (ROOT / ".github/workflows/tor-http-load.yml").read_text()
        events = text.split("\npermissions:", 1)[0] + "\n"
        self.assertIn("  pull_request:\n    paths:\n", events)
        self.assertIn("  workflow_dispatch:\n", events)
        self.assertNotIn("  push:\n", events)
        self.assertEqual(text.count("\nconcurrency:\n"), 1)
        self.assertIn(
            "concurrency:\n"
            "  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.run_id }}\n"
            "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n",
            text,
        )


if __name__ == "__main__":
    unittest.main()
