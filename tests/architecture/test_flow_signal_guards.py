"""Signal and public-page reports require fresh authority and bounded read-only evidence."""
from pathlib import Path
import json
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_governance import check


class SignalGuardTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for source in (ROOT / "crates").glob("*/src/**/*.rs"):
            target = self.root / source.relative_to(ROOT)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        target = self.root / "docs/contracts/ril-route-guards-v1.json"
        target.parent.mkdir(parents=True)
        shutil.copy2(ROOT / "docs/contracts/ril-route-guards-v1.json", target)

    def test_live_authorization_and_deadline_cannot_be_removed(self):
        for child in ("signals", "public_pages"):
            path = self.root / f"crates/web/src/auth/audience_http/{child}.rs"
            original = path.read_text()
            for marker in (
                "administrator(&state, &headers)",
                "Ok(Err(status)) => return secure(status)",
                "Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE)",
                "Duration::from_millis(950)",
                "timeout_at(deadline, store.report(",
                "secure((status, render::page(",
            ):
                with self.subTest(child=child, marker=marker):
                    self.assertIn(marker, original)
                    path.write_text(original.replace(marker, "UNGUARDED", 1))
                    errors = check(self.root)
                    self.assertTrue(any("/admin/audience/" in item for item in errors), errors)
            path.write_text(original)

    def test_report_policy_cannot_be_downgraded(self):
        path = self.root / "docs/contracts/ril-route-guards-v1.json"
        original = path.read_text()
        for route in ("/admin/audience/signals", "/admin/audience/pages"):
            with self.subTest(route=route):
                data = json.loads(original)
                next(row for row in data["routes"] if row["template"] == route)["policy"] = "session"
                path.write_text(json.dumps(data))
                self.assertTrue(any(route in item for item in check(self.root)))
