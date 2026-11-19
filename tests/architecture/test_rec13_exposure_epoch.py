"""REC/FLOW exposure browser must use a fresh, verified collector epoch.

The strict HTTP 403 on a withdrawn book is never replaced by a 503 allowance.
"""
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class ExposureEpochIsolationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.script = (ROOT / "tools/rec13_browser_acceptance.sh").read_text()
        cls.browser = (ROOT / "tools/exposure_pg_browser.cjs").read_text()

    def test_fresh_worker_precedes_strict_exposure_browser(self):
        script = self.script
        start = script.index("old_exposure_epoch=")
        browser = script.index("node tools/exposure_pg_browser.cjs")
        self.assertLess(start, browser)
        isolated = script[start:browser]
        self.assertIn("stop_server\nmv \"$root/server-current.log\" \"$root/server-current-before-exposure.log\"", isolated)
        self.assertIn('start_server "$TEST_DATABASE_URL" current', isolated)
        self.assertIn("new_exposure_epoch=", isolated)
        self.assertIn("new_exposure_epoch <= old_exposure_epoch", isolated)
        self.assertIn("CI_EXPOSURE_FRESH_EPOCH_OK", isolated)
        self.assertIn("NOT retired", isolated.replace("not retired", "NOT retired"))
        self.assertIn("lease_until_ms>", isolated)
        self.assertIn("confirmed_ms>=", isolated)
        self.assertIn('[[ "$exposure_epoch_ready" == t ]]', isolated)
        self.assertNotIn("|| true", isolated)
        self.assertNotIn("retry", isolated.split("node --test tools/exposure_client.test.cjs")[-1])

    def test_real_withdrawal_still_requires_http_403(self):
        browser = self.browser
        self.assertIn("UPDATE books SET admin_hold=true WHERE id=${book}", browser)
        self.assertIn(".status(),403", browser)
        self.assertNotIn(".status(),503", browser)


if __name__ == "__main__":
    unittest.main()
