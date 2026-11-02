"""Mutation tests for TOR roadmap audit: a green check must mean intact coverage."""
import pathlib
import shutil
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from check_roadmap import check

class RoadmapTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        (self.root / "docs" / "roadmap").mkdir(parents=True)
        for name in ("scenarios.tsv", "tasks.tsv", "sources.lock.toml", "api-v1.md", "functions-coverage-audit.tsv"):
            shutil.copyfile(ROOT / "docs" / "roadmap" / name, self.root / "docs" / "roadmap" / name)

    def mutate(self, name, old, new):
        path = self.root / "docs" / "roadmap" / name
        text = path.read_text(encoding="utf-8")
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    def test_complete_plan(self):
        self.assertEqual(check(self.root), [])

    def test_missing_scenario_detected(self):
        self.mutate("scenarios.tsv", "S021\tB", "S070\tB")
        self.assertTrue(any("S001..S069" in e for e in check(self.root)))

    def test_yandex_oauth_scenario_stays_frozen(self):
        self.mutate("scenarios.tsv", "S003\t-\tзаморожено", "S003\tA\tСохранить")
        self.assertTrue(any("S003" in issue for issue in check(self.root)))

    def test_yandex_oauth_task_stays_archived(self):
        self.mutate("tasks.tsv", "A-08\tA\tЗАМОРОЖЕНО", "A-08\tA\t· Д4 P1")
        self.assertTrue(any("A-08" in issue for issue in check(self.root)))

    def test_cross_owner_misrouting_detected(self):
        self.mutate("scenarios.tsv", "S032\tD", "S032\tB")
        self.assertTrue(any("S032" in e for e in check(self.root)))

    def test_missing_contract_detected(self):
        self.mutate("scenarios.tsv", "POST /register; POST /login; POST /logout", "")
        self.assertTrue(any("missing chosen TOR entrypoint" in e for e in check(self.root)))

    def test_missing_function_detected(self):
        self.mutate("functions-coverage-audit.tsv", "F031\t", "F131\t")
        self.assertTrue(any("F001..F061" in e for e in check(self.root)))

    def test_insufficient_function_must_have_gap(self):
        p = self.root / "docs" / "roadmap" / "functions-coverage-audit.tsv"
        text = p.read_text(encoding="utf-8").splitlines(keepends=True)
        head = next(i for i, row in enumerate(text) if row.startswith("F031\t"))
        vals = text[head].rstrip("\n").split("\t")
        vals[6] = ""
        text[head] = "\t".join(vals) + "\n"
        p.write_text("".join(text), encoding="utf-8")
        self.assertTrue(any("missing concrete implementation/prelaunch gap" in e for e in check(self.root)))

    def test_missing_task_detected(self):
        p = self.root / "docs" / "roadmap" / "tasks.tsv"
        rows = p.read_text(encoding="utf-8").splitlines(keepends=True)
        p.write_text("".join(x for x in rows if not x.startswith("B-03\t")), encoding="utf-8")
        self.assertTrue(any("expected 117 tasks" in e for e in check(self.root)))

    def test_unrun_test_selector_detected(self):
        self.mutate("tasks.tsv", "cargo test -p tor-accounts --test slices_pg a_03_",
                    "cargo test -p tor-accounts --test slices_pg something_else")
        self.assertTrue(any("deterministic test selector" in e for e in check(self.root)))

    def test_vague_instruction_detected(self):
        self.mutate("tasks.tsv", "Перенести регистрацию:", "Согласовать регистрацию:")
        self.assertTrue(any("vague planning instruction" in e for e in check(self.root)))

    def test_source_sha_drift_detected(self):
        self.mutate("scenarios.tsv", "17125ef758a791e9630505c14d24f7fe65aed228", "0000000000000000000000000000000000000000")
        self.assertTrue(any("source link is not pinned" in e for e in check(self.root)))

    def test_json_duplicate_idempotency_field_detected(self):
        self.mutate("api-v1.md", "{chapter_id,expected_version_id,expected_price_minor}",
                    "{chapter_id,expected_version_id,expected_price_minor,idempotency_key}")
        self.assertTrue(any("Idempotency-Key header" in e for e in check(self.root)))

    def test_placeholder_detected(self):
        self.mutate("tasks.tsv", "Перенести регистрацию:", "TODO Перенести регистрацию:")
        self.assertTrue(any("placeholder" in e for e in check(self.root)))

    def edit_scenario_fields(self, code, **updates):
        path = self.root / "docs" / "roadmap" / "scenarios.tsv"
        rows = path.read_text(encoding="utf-8").splitlines()
        header = next(row.split("\t") for row in rows if row.startswith("code\t"))
        for index, row in enumerate(rows):
            if row.startswith(code + "\t"):
                values = row.split("\t")
                for field, value in updates.items():
                    values[header.index(field)] = value
                rows[index] = "\t".join(values)
                break
        path.write_text("\n".join(rows) + "\n", encoding="utf-8")

    def test_verified_behavior_can_be_limited_to_a_draft(self):
        self.edit_scenario_fields("S035", rust_scope="draft")
        self.assertEqual(check(self.root), [])

    def test_verified_snapshot_requires_evidence_and_scope(self):
        self.edit_scenario_fields("S035", rust_evidence_urls="", rust_scope="")
        errors = check(self.root)
        self.assertTrue(any("Rust status or scope" in e for e in errors))
        self.assertTrue(any("Rust evidence URLs" in e for e in errors))

    def test_missing_snapshot_date_and_revision_rejected(self):
        self.edit_scenario_fields("S001", rust_checked_at="2026-02-30", rust_snapshot_sha="main")
        errors = check(self.root)
        self.assertTrue(any("Rust check date" in e for e in errors))
        self.assertTrue(any("Rust main snapshot SHA" in e for e in errors))

    def test_code_cannot_have_not_started_scope(self):
        self.edit_scenario_fields("S002", rust_scope="not_started")
        self.assertTrue(any("contradicts not_started" in e for e in check(self.root)))

    def test_excluded_snapshot_cannot_claim_main(self):
        self.edit_scenario_fields("S024", rust_scope="main")
        self.assertTrue(any("excluded Rust status/scope" in e for e in check(self.root)))

if __name__ == "__main__":
    unittest.main()
