"""The shared editorial placements keep a separate administrator write boundary."""
from pathlib import Path
import json
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_governance import check


class EditorialGuardTests(unittest.TestCase):
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

    def test_current_boundary_is_reviewed(self):
        self.assertEqual(check(self.root), [])

    def test_live_role_csrf_and_actor_binding_cannot_be_removed(self):
        mutations = (
            ("mod.rs", "!actor.administrator"),
            ("admin.rs", "state.cookies.same_origin(&headers)"),
            ("admin.rs", "state.cookies.valid_csrf(&headers, &form.csrf_token)"),
            ("admin.rs", ".execute(&actor, &form.command_key, &command)"),
            ("admin.rs", "secure(StatusCode::SERVICE_UNAVAILABLE, &[])"),
        )
        for filename, marker in mutations:
            with self.subTest(marker=marker):
                path = self.root / "crates/web/src/editorial_http" / filename
                original = path.read_text()
                self.assertIn(marker, original)
                path.write_text(original.replace(marker, "UNGUARDED", 1))
                self.assertTrue(any("/admin/promotion/editorial" in row for row in check(self.root)))
                path.write_text(original)

    def test_inventory_cannot_downgrade_either_method(self):
        path = self.root / "docs/contracts/ril-route-guards-v1.json"
        original = path.read_text()
        for method in ("GET", "POST"):
            with self.subTest(method=method):
                data = json.loads(original)
                next(row for row in data["routes"] if row["template"] == "/admin/promotion/editorial"
                     and row["method"] == method)["policy"] = "session"
                path.write_text(json.dumps(data))
                self.assertTrue(any("live administrator" in row for row in check(self.root)))
