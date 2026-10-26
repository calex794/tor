"""Mutation checks: an attractive roadmap cannot silently drop critical contracts."""
from __future__ import annotations

import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_contracts import check  # noqa: E402


class ContractTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        for folder in ("docs/contracts", "docs/roadmap"):
            shutil.copytree(ROOT / folder, self.root / folder)

    def update_json(self, name: str, mutator):
        path = self.root / "docs" / "contracts" / name
        val = json.loads(path.read_text(encoding="utf-8"))
        mutator(val)
        path.write_text(json.dumps(val, ensure_ascii=False), encoding="utf-8")

    def test_full_contract_set(self):
        self.assertEqual([], check(self.root))

    def test_disappeared_paid_s3_path(self):
        self.update_json("openapi-v1.json", lambda data: data["paths"].pop(
            "/api/v1/reader/chapters/{chapter_id}/delivery-grants"))
        self.assertTrue(any("missing high-risk API" in x for x in check(self.root)))

    def test_reintroduced_paid_plaintext(self):
        self.update_json("openapi-v1.json", lambda data: data["components"]["schemas"]["ChapterMetadata"]["properties"].update(
            {"body_markdown": {"type": "string"}}))
        self.assertTrue(any("plaintext" in x for x in check(self.root)))

    def test_reintroduced_url_crawler(self):
        self.update_json("openapi-v1.json", lambda data: data["components"]["schemas"]["ImportCreate"]["properties"]["mode"].update(
            {"const": "url"}))
        self.assertTrue(any("crawler mode=url" in x for x in check(self.root)))

    def test_money_flag_cannot_default_on(self):
        self.update_json("release-flags-v1.json", lambda data: data["flags"]["chapter_ads_billable_enabled"].update(
            {"default_enabled": True}))
        self.assertTrue(any("chapter_ads_billable_enabled" in x for x in check(self.root)))

    def test_recommendation_wrong_total(self):
        self.update_json("rec-golden-v1.json", lambda data: data["test_cases"][0]["expected"]["ordered"][0].update(
            {"final_score": 999}))
        self.assertTrue(any("scoring oracle mismatch" in x for x in check(self.root)))

    def test_s024_scope_stays_frozen(self):
        path = self.root / "docs" / "roadmap" / "scenarios.tsv"
        val = path.read_text(encoding="utf-8")
        self.assertIn("S024\t-\tзаморожено", val)
        path.write_text(val.replace("S024\t-\tзаморожено", "S024\tG\tСохранить", 1), encoding="utf-8")
        self.assertTrue(any("S024 crawler" in x for x in check(self.root)))

    def test_unresolved_component_reference(self):
        self.update_json("openapi-v1.json", lambda data: data["components"]["schemas"].pop("DeliveryGrant"))
        self.assertTrue(any("unresolved OpenAPI reference" in x for x in check(self.root)))

    def test_money_idempotency_header_is_required(self):
        self.update_json("openapi-v1.json", lambda data: data["paths"]["/api/v1/purchases"]["post"]["parameters"].clear())
        self.assertTrue(any("Idempotency-Key header" in x for x in check(self.root)))

    def test_missing_ril_platform_document(self):
        path = self.root / "docs" / "contracts" / "ril-platform-v1.md"
        path.unlink()
        self.assertTrue(any("ril-platform-v1.md" in x for x in check(self.root)))

    def test_ril_stage_deleted_from_platform(self):
        path = self.root / "docs" / "contracts" / "ril-platform-v1.md"
        value = path.read_text(encoding="utf-8")
        path.write_text(value.replace("RIL-06", "RIL-XX"), encoding="utf-8")
        self.assertTrue(any("RIL platform missing stage RIL-06" in x for x in check(self.root)))

    def test_ril_aggregate_task_cannot_disappear(self):
        path = self.root / "docs" / "roadmap" / "tasks.tsv"
        value = path.read_text(encoding="utf-8")
        path.write_text(value.replace("RIL-06", "RIL-XX"), encoding="utf-8")
        self.assertTrue(any("RIL task G-07 missing" in x for x in check(self.root)))

    def test_ril_route_governance_task_cannot_disappear(self):
        path = self.root / "docs" / "roadmap" / "tasks.tsv"
        value = path.read_text(encoding="utf-8")
        path.write_text(value.replace("RIL-09", "RIL-XX"), encoding="utf-8")
        self.assertTrue(any("RIL task I-07 missing" in x for x in check(self.root)))

    def test_missing_policy_document(self):
        path = self.root / "docs" / "contracts" / "refunds-payouts-v1.md"
        path.unlink()
        self.assertTrue(any("refunds-payouts-v1.md" in x for x in check(self.root)))


if __name__ == "__main__":
    unittest.main()
