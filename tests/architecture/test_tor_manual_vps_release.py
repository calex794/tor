"""Guard the real manual-only TOR VPS workflow without contacting a VPS."""

from pathlib import Path
import os
import subprocess
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = (ROOT / ".github/workflows/tor-vps-staging.yml").read_text(
    encoding="utf-8"
)
START = "      - name: Validate target and explicit installation approval\n        run: |\n"
END = "      - uses: actions/checkout@v4\n"
GOOD_SHA = "a" * 40
OTHER_SHA = "b" * 40


def approval_shell() -> str:
    """Test the exact script deployed in the workflow, not a reimplementation."""
    if WORKFLOW.count(START) != 1:
        raise AssertionError("missing or duplicate release approval step")
    body = WORKFLOW.split(START, 1)[1].split(END, 1)[0]
    return textwrap.dedent(body)


def run_approval(install: bool, confirmation: str, sha: str = GOOD_SHA):
    env = os.environ.copy()
    env.update({
        "TARGET_SHA": sha,
        "AUTO_INSTALL": "true" if install else "false",
        "INSTALL_CONFIRM_SHA": confirmation,
        "TOR_DEPLOY_HOST": "localhost",
        "TOR_DEPLOY_USER": "tor-deploy",
        "TOR_DEPLOY_SSH_KEY": "fake-key-for-local-test",
        "TOR_DEPLOY_KNOWN_HOSTS": "fake-host-for-local-test",
    })
    return subprocess.run(
        ["bash", "-euo", "pipefail", "-c", approval_shell()],
        capture_output=True,
        text=True,
        env=env,
        timeout=5,
    )


class ManualVpsReleaseTests(unittest.TestCase):
    def test_only_manual_run_on_main_may_contact_vps(self):
        self.assertIn("on:\n  workflow_dispatch:", WORKFLOW)
        self.assertNotIn("  workflow_run:", WORKFLOW)
        self.assertNotIn("  push:", WORKFLOW)
        self.assertNotIn("TOR_AUTO_DEPLOY", WORKFLOW)
        self.assertIn(
            "if: github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'",
            WORKFLOW,
        )
        self.assertIn("TARGET_SHA: ${{ github.sha }}", WORKFLOW)
        self.assertIn("AUTO_INSTALL: ${{ inputs.install }}", WORKFLOW)
        self.assertIn("INSTALL_CONFIRM_SHA: ${{ inputs.confirm_sha }}", WORKFLOW)

    def test_verification_precedes_checkout_and_transfer(self):
        self.assertLess(
            WORKFLOW.index(START),
            WORKFLOW.index("      - uses: actions/checkout@v4"),
        )
        self.assertLess(
            WORKFLOW.index("Validate target and explicit installation approval"),
            WORKFLOW.index("Transfer verified production binaries"),
        )
        self.assertIn("Verify current main is the selected release", WORKFLOW)
        self.assertIn("Refuse obsolete main release before switching active binary", WORKFLOW)

    def test_stage_without_confirmation_is_allowed(self):
        result = run_approval(False, "")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_stage_with_confirmation_is_refused(self):
        result = run_approval(False, GOOD_SHA)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("REFUSE_STAGE:", result.stderr)

    def test_install_with_exact_current_sha_is_allowed(self):
        result = run_approval(True, GOOD_SHA)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_install_with_mismatched_or_partial_sha_is_refused(self):
        for confirmation in ("", OTHER_SHA, GOOD_SHA[:12], GOOD_SHA + "0"):
            with self.subTest(confirmation=confirmation):
                result = run_approval(True, confirmation)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("REFUSE_INSTALL:", result.stderr)

    def test_invalid_target_sha_is_refused_even_for_staging(self):
        for sha in ("not-a-sha", "A" * 40, "a" * 39):
            with self.subTest(sha=sha):
                self.assertNotEqual(run_approval(False, "", sha).returncode, 0)


if __name__ == "__main__":
    unittest.main()
