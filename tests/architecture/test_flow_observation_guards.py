"""Anonymous proof collection must not turn into an unguarded write or admin read."""
from pathlib import Path
import json
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_governance import check


class ObservationGuardTests(unittest.TestCase):
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

    def test_origin_proof_live_rights_and_budget_are_required(self):
        path = self.root / "crates/web/src/flow_exposure/ingest.rs"
        original = path.read_text()
        for marker in (
            "state.cookies.same_origin(&headers)",
            '"x-tor-observation"',
            "state.store.describe(&event.proof)",
            "if !guard",
            ".allowed(&context.card)",
            ".active_session_user(&session_token_digest(&token))",
            "state.store.record(&event, &context, actor)",
            "Duration::from_millis(950)",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, original)
                path.write_text(original.replace(marker, "UNGUARDED", 1))
                self.assertTrue(any("server-proof observation guard" in item for item in check(self.root)))

    def test_admin_role_failure_and_timeout_cannot_be_ignored(self):
        path = self.root / "crates/web/src/flow_exposure/report.rs"
        original = path.read_text()
        for marker in (
            "!actor.administrator",
            "timeout_at(deadline, guard.campaign_book(campaign))",
            "timeout_at(deadline, query)",
            "Ok(Err(status)) => return secure(status)",
            "Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE)",
        ):
            with self.subTest(marker=marker):
                self.assertIn(marker, original)
                path.write_text(original.replace(marker, "UNGUARDED", 1))
                self.assertTrue(any("bounded live administrator read" in item for item in check(self.root)))

    def test_policies_cannot_be_downgraded(self):
        path = self.root / "docs/contracts/ril-route-guards-v1.json"
        original = path.read_text()
        for route, policy in (("/api/v1/flow/observations", "public"),
                              ("/admin/audience/exposure", "session")):
            with self.subTest(route=route):
                data = json.loads(original)
                next(row for row in data["routes"] if row["template"] == route)["policy"] = policy
                path.write_text(json.dumps(data))
                self.assertTrue(any(route in item for item in check(self.root)))
