"""Offline NEwBOO -> TOR taxonomy snapshot completeness and collisions."""
import json
import sys
from pathlib import Path
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import newboo_taxonomy_bridge as bridge

A = "11111111-1111-4111-8111-111111111111"
B = "22222222-2222-4222-8222-222222222222"
C = "33333333-3333-4333-8333-333333333333"

def fixture():
    return {"source": bridge.SOURCE, "expected_counts": {"taxonomies": 3, "aliases": 2},
            "taxonomies": [
                {"taxonomy_id": A, "taxonomy_type": "tag", "title": "Система", "status": "active"},
                {"taxonomy_id": B, "taxonomy_type": "genre", "title": "Фэнтези", "status": "active"},
                {"taxonomy_id": C, "taxonomy_type": "tag", "title": "Старый", "status": "merged"}],
            "aliases": [
                {"alias_id": "al-1", "taxonomy_id": A, "taxonomy_type": "tag", "alias": "System"},
                {"alias_id": "al-2", "taxonomy_id": A, "taxonomy_type": "tag", "alias": "系统"}]}

class BridgeTests(unittest.TestCase):
    def write(self, data):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = Path(tmp.name) / "full.json"
        path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        return path

    def test_only_active_snapshot_to_candidates(self):
        data = bridge.load(self.write(fixture()))
        terms, report = bridge.prepare(data)
        self.assertEqual(len(terms), 2)
        self.assertEqual(report["stats"]["candidate_aliases"], 2)
        self.assertEqual(report["fatal_errors"], [])
        self.assertEqual(len(report["manual_review"]), 1)

    def test_reject_admin_api_and_incomplete_count(self):
        data = fixture()
        data["source"] = "admin-api"
        with self.assertRaisesRegex(ValueError, "full independent"):
            bridge.load(self.write(data))
        data = fixture()
        data["expected_counts"]["taxonomies"] = 1000
        with self.assertRaisesRegex(ValueError, "count mismatch"):
            bridge.load(self.write(data))

    def test_alias_to_wrong_kind_is_fatal(self):
        data = fixture()
        data["aliases"][0]["taxonomy_type"] = "genre"
        _, report = bridge.prepare(data)
        self.assertTrue(report["fatal_errors"])

    def test_source_canonical_alias_collision_is_fatal(self):
        data = fixture()
        data["taxonomies"].append({
            "taxonomy_id": "44444444-4444-4444-8444-444444444444",
            "taxonomy_type": "tag", "title": "System", "status": "active"})
        data["expected_counts"]["taxonomies"] += 1
        _, report = bridge.prepare(data)
        self.assertTrue(any("collides" in e for e in report["fatal_errors"]))

    def test_conflict_writes_report_but_never_creates_candidate(self):
        src = fixture()
        src["taxonomies"].append({
            "taxonomy_id": "44444444-4444-4444-8444-444444444444",
            "taxonomy_type": "tag", "title": "System", "status": "active"})
        src["expected_counts"]["taxonomies"] += 1
        source = self.write(src)
        output = source.parent / "candidate.csv"
        report = source.parent / "review.json"
        self.assertEqual(bridge.main([
            str(source), "--out", str(output), "--report", str(report)]), 2)
        self.assertFalse(output.exists())
        self.assertTrue(report.exists())
        self.assertTrue(json.loads(report.read_text(encoding="utf-8"))["fatal_errors"])

    def test_successful_conversion_is_accepted_by_real_tor_importer(self):
        source = self.write(fixture())
        output = source.parent / "candidate.csv"
        report = source.parent / "review.json"
        self.assertEqual(bridge.main([
            str(source), "--out", str(output), "--report", str(report)]), 0)
        from taxonomy_import_csv import checked_csv
        rows = checked_csv(str(output))
        self.assertEqual(len(rows), 2)
        self.assertEqual(len(json.loads(report.read_text(encoding="utf-8"))["manual_review"]), 1)

    def test_no_input_or_report_can_be_overwritten_as_candidate(self):
        source = self.write(fixture())
        self.assertEqual(bridge.main([
            str(source), "--out", str(source), "--report", str(source.parent / "r.json")]), 2)
        self.assertEqual(bridge.load(source)["expected_counts"]["taxonomies"], 3)

    def test_different_normalization_semantics_are_reported_not_merged(self):
        self.assertEqual(bridge.tor_key("Ёжик"), "ежик")
        self.assertEqual(bridge.semantic("гг"), "главный герой")
        self.assertEqual(bridge.semantic(" Ｓｙｓｔｅｍ "), "system")
        self.assertNotEqual(bridge.tor_key("гг"), bridge.tor_key("Главный герой"))

if __name__ == "__main__":
    unittest.main()
