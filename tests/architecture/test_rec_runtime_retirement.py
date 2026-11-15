"""Retired endpoints keep authority guards and cannot disguise fresh collection as 410."""
from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_ril_governance import check  # noqa: E402

API = "crates/web/src/recommendation_http/api.rs"
PAGES = "crates/web/src/recommendation_http/pages.rs"
ORGANIC = "crates/web/src/recommendation_http/organic.rs"
INVENTORY = "docs/contracts/ril-route-guards-v1.json"


class RetiredCollectionGuards(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        for src in (ROOT / "crates").glob("*/src/**/*.rs"):
            self.copy(src)
        self.copy(ROOT / INVENTORY)

    def copy(self, src):
        path = self.root / src.relative_to(ROOT)
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, path)

    def mutate(self, path, before, after):
        target = self.root / path
        source = target.read_text(encoding="utf-8")
        self.assertIn(before, source)
        target.write_text(source.replace(before, after, 1), encoding="utf-8")

    def assert_retirement_rejected(self):
        findings = check(self.root)
        self.assertTrue(any("retired collection route" in value for value in findings), findings)

    def test_retired_route_does_not_hide_a_repository_write(self):
        self.mutate(
            PAGES, "    api::retired()\n}",
            '    let _ = state.store.set_consent(1, true, 0, "test").await;\n'
            '    api::retired()\n}',
        )
        self.assert_retirement_rejected()

    def test_retired_response_cannot_report_success(self):
        self.mutate(API, "StatusCode::GONE,", "StatusCode::OK,")
        self.assert_retirement_rejected()

    def test_retired_response_preserves_error_code(self):
        self.mutate(API, 'code: "COLLECTION_RETIRED"', 'code: "UNAVAILABLE"')
        self.assert_retirement_rejected()

    def test_retired_response_cannot_become_cacheable(self):
        target = self.root / API
        source = target.read_text(encoding="utf-8")
        start = source.index("pub(crate) fn retired()")
        end = source.index("\nfn idempotency_error", start)
        part = source[start:end]
        self.assertIn("private_response(", part)
        part = part.replace("private_response(", "public_response(", 1)
        target.write_text(source[:start] + part + source[end:], encoding="utf-8")
        self.assert_retirement_rejected()

    def test_retired_scope_policy_cannot_be_downgraded(self):
        target = self.root / INVENTORY
        rows = json.loads(target.read_text())
        row = next(row for row in rows["routes"]
                   if row["template"] == "/me/recommendations/flow-consent")
        row["policy"] = "session"
        target.write_text(json.dumps(rows), encoding="utf-8")
        self.assert_retirement_rejected()

    def test_retired_cross_module_actor_failure_must_return(self):
        self.mutate(
            ORGANIC,
            "if let Err(status) = api::actor(&state, &headers).await {\n        return deny(status);\n    }",
            "let _ = api::actor(&state, &headers).await;",
        )
        self.assert_retirement_rejected()

    def test_retired_cross_module_csrf_helper_must_be_called(self):
        self.mutate(ORGANIC, "api::csrf(&state", "unchecked_csrf(&state")
        findings = check(self.root)
        self.assertTrue(any(
            "/api/v1/discovery/activate" in value and "missing Origin+CSRF rejection" in value
            for value in findings
        ), findings)

    def test_retired_a_guard_cannot_accept_anonymous_session(self):
        self.mutate(
            "crates/web/src/auth/behavior_consent.rs",
            "Ok(None) => deny(StatusCode::UNAUTHORIZED)",
            "Ok(None) => crate::recommendation_http::retired()",
        )
        self.assert_retirement_rejected()


if __name__ == "__main__":
    unittest.main()
