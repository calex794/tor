"""Mutation tests for locked NEwBOO route and worker coverage snapshots."""
from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_legacy_coverage import audit


class LegacyCoverageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        out = self.root / "docs" / "roadmap" / "legacy-inventory"
        out.mkdir(parents=True)
        source = ROOT / "docs" / "roadmap"
        shutil.copy2(source / "sources.lock.toml", out.parent / "sources.lock.toml")
        for file in (source / "legacy-inventory").glob("*"):
            shutil.copy2(file, out / file.name)

    def edit(self, name: str, old: str, new: str) -> None:
        path = self.root / "docs" / "roadmap" / "legacy-inventory" / name
        s = path.read_text(encoding="utf-8")
        self.assertIn(old, s)
        path.write_text(s.replace(old, new, 1), encoding="utf-8")

    def test_consistent_snapshot(self):
        problems, stats = audit(self.root)
        self.assertEqual([], problems)
        self.assertEqual(stats["source_declarations"], 560)
        self.assertEqual(stats["job_registered"], 57)
        self.assertEqual(stats["job_frozen"], 8)
        self.assertEqual(stats["review_unresolved_mount"], 1)

    def test_strict_does_not_claim_unreviewed_routes(self):
        problems, _ = audit(self.root, strict=True)
        self.assertTrue(any("semantic mapping" in x for x in problems))
        self.assertTrue(any("UNMOUNTED/UNVERIFIED" in x for x in problems))

    def test_missing_decorator_detected(self):
        f = "routes-part-000.tsv"
        self.edit(f, "ads_ping", "renamed_handler")
        problems, _ = audit(self.root)
        self.assertTrue(any("inventory mismatch" in x for x in problems))

    def test_missing_mounted_route_detected(self):
        f = "http-route-coverage.tsv"
        self.edit(f, "ads_ping", "missing_ping")
        problems, _ = audit(self.root)
        self.assertTrue(any("inventory mismatch" in x for x in problems))

    def test_crawler_freeze_cannot_be_lifted(self):
        f = "http-route-coverage.tsv"
        self.edit(f, "frozen\t-\tF021/S024", "mounted\tG\tG-03")
        problems, _ = audit(self.root)
        self.assertTrue(any("frozen" in x for x in problems))

    def test_worker_freeze_cannot_be_lifted(self):
        f = "deferred-jobs.tsv"
        self.edit(f, "scene_marketplace_notification_event\t", "test_scene_marketplace_notification_event\t")
        problems, _ = audit(self.root)
        self.assertTrue(any("frozen deferred jobs" in x or "freeze" in x for x in problems))

    def test_source_pin_failure(self):
        f = "routes-part-000.tsv"
        self.edit(f, "17125ef758a791e9630505c14d24f7fe65aed228",
                     "ffffffffffffffffffffffffffffffffffffffff")
        problems, _ = audit(self.root)
        self.assertTrue(any("source pin" in x for x in problems))


if __name__ == "__main__":
    unittest.main()
