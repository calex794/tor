"""Manual taxonomy review decisions -> preflighted importer CSV; no database."""
import contextlib
import csv
import io
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import taxonomy_import_csv as importer
import taxonomy_review_compile as compiler
import taxonomy_review_export as exporter


class ReviewCompilationTests(unittest.TestCase):
    @staticmethod
    def candidate():
        return {
            "kind": "tag", "normalized_key": "система", "review_label": "Система",
            "public_uses": 3, "spelling_count": 1, "term_id": 42,
            "status": "candidate", "raw_spellings": [{"label": "Система", "uses": 3}],
        }

    @staticmethod
    def not_staged():
        return {
            "kind": "tag", "normalized_key": "попаданец", "review_label": "Попаданец",
            "public_uses": 2, "spelling_count": 1, "term_id": None,
            "status": "not_staged", "raw_spellings": [{"label": "Попаданец", "uses": 2}],
        }

    def review_file(self, root, items):
        path = Path(root) / "review.csv"
        exporter.export_review(items, path)
        return path

    def edit(self, source, replacements):
        with source.open(encoding="utf-8-sig", newline="") as handle:
            reader = csv.DictReader(handle)
            rows = list(reader)
        for i, changes in replacements.items():
            rows[i].update(changes)
        source.unlink()
        with source.open("w", encoding="utf-8-sig", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=exporter.REVIEW_COLUMNS)
            writer.writeheader()
            writer.writerows(rows)
        return source

    def test_two_explicit_approvals_compile_only_to_import_format(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate(), self.not_staged()])
            self.edit(src, {
                0: {"decision": "approve", "approved_label": "Система",
                    "approved_aliases": "System|系统"},
                1: {"decision": "approve", "approved_label": "Попаданец",
                    "approved_aliases": "Otherworld Traveler"},
            })
            dest = Path(root) / "approved.csv"
            with mock.patch.object(compiler, "checked_csv", wraps=importer.checked_csv) as check:
                count, total = compiler.compile_review(src, dest)
            self.assertEqual((count, total), (2, 2))
            self.assertEqual(check.call_count, 1)
            self.assertEqual(importer.checked_csv(dest), [
                ("tag", "Система", ["System", "系统"]),
                ("tag", "Попаданец", ["Otherworld Traveler"]),
            ])
            self.assertEqual(dest.open(encoding="utf-8-sig").readline().strip(),
                             "kind,label,aliases")

    def test_skip_requires_explicit_decision_and_makes_no_implicit_approval(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate(), self.not_staged()])
            dest = Path(root) / "approved.csv"
            with self.assertRaisesRegex(ValueError, "missing explicit"):
                compiler.compile_review(src, dest)
            self.assertFalse(dest.exists())
            self.edit(src, {
                0: {"decision": "approve", "approved_label": "Система"},
                1: {"decision": "skip"},
            })
            self.assertEqual(compiler.compile_review(src, dest), (1, 2))
            self.assertEqual(importer.checked_csv(dest), [("tag", "Система", [])])

    def test_skip_cannot_sneak_approval_values(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate()])
            self.edit(src, {0: {"decision": "skip", "approved_label": "Система"}})
            with self.assertRaisesRegex(ValueError, "skipped review"):
                compiler.read_decisions(src)

    def test_existing_candidate_must_keep_exact_name_and_id(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "System"}})
            with self.assertRaisesRegex(ValueError, "retain its canonical"):
                compiler.read_decisions(src)
            self.edit(src, {0: {"approved_label": "Система", "term_id": "0"}})
            with self.assertRaisesRegex(ValueError, "term_id"):
                compiler.read_decisions(src)

    def test_blocked_or_merged_cannot_be_auto_approved(self):
        for status in ("blocked", "merged"):
            with self.subTest(status=status):
                with TemporaryDirectory() as root:
                    row = self.candidate()
                    row["status"] = status
                    src = self.review_file(root, [row])
                    self.edit(src, {0: {"decision": "approve", "approved_label": "Система"}})
                    with self.assertRaisesRegex(ValueError, "blocked/merged"):
                        compiler.read_decisions(src)

    def test_changed_new_label_needs_explicit_legacy_alias(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.not_staged()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "Иномирец"}})
            with self.assertRaisesRegex(ValueError, "among approved aliases"):
                compiler.read_decisions(src)
            self.edit(src, {0: {"approved_aliases": "Попаданец"}})
            dest = Path(root) / "approved.csv"
            compiler.compile_review(src, dest)
            self.assertEqual(importer.checked_csv(dest),
                             [("tag", "Иномирец", ["Попаданец"])])

    def test_missing_snapshot_row_fails_even_if_remaining_ones_approved(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate(), self.not_staged()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "Система"},
                            1: {"decision": "skip"}})
            with src.open(encoding="utf-8-sig", newline="") as handle:
                data = list(csv.DictReader(handle))
            src.unlink()
            with src.open("w", encoding="utf-8-sig", newline="") as handle:
                writer = csv.DictWriter(handle, fieldnames=exporter.REVIEW_COLUMNS)
                writer.writeheader()
                writer.writerow(data[0])
            with self.assertRaisesRegex(ValueError, "incomplete review"):
                compiler.read_decisions(src)

    def test_tampering_spellings_or_counts_fails(self):
        for change in (
            {"public_uses": "999"},
            {"spelling_count": "2"},
            {"normalized_key": "ситема"},
            {"raw_spellings_json": "[]"},
            {"kind": "invalid"},
        ):
            with self.subTest(change=change):
                with TemporaryDirectory() as root:
                    src = self.review_file(root, [self.candidate()])
                    self.edit(src, {0: {"decision": "approve",
                                       "approved_label": "Система", **change}})
                    with self.assertRaises(ValueError):
                        compiler.read_decisions(src)

    def test_duplicate_groups_and_colliding_aliases_refused(self):
        with TemporaryDirectory() as root:
            row = self.candidate()
            other = dict(row, term_id=43)
            src = self.review_file(root, [row, other])
            self.edit(src, {0: {"decision": "approve", "approved_label": "Система"},
                            1: {"decision": "skip"}})
            with self.assertRaisesRegex(ValueError, "duplicate review"):
                compiler.read_decisions(src)

        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate(), self.not_staged()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "Система",
                                "approved_aliases": "Shared"},
                            1: {"decision": "approve", "approved_label": "Попаданец",
                                "approved_aliases": "Shared"}})
            dest = Path(root) / "approved.csv"
            with self.assertRaisesRegex(ValueError, "colliding"):
                compiler.compile_review(src, dest)
            self.assertFalse(dest.exists())

    def test_formula_approval_refused_and_safe_encoded_review_name_supported(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "=1+1"}})
            with self.assertRaisesRegex(ValueError, "spreadsheet formula"):
                compiler.read_decisions(src)
        with TemporaryDirectory() as root:
            row = self.not_staged()
            row.update(normalized_key="=1+1", review_label="=1+1",
                       raw_spellings=[{"label": "=1+1", "uses": 2}])
            src = self.review_file(root, [row])
            self.edit(src, {0: {"decision": "skip"}})
            with self.assertRaisesRegex(ValueError, "no explicit approvals"):
                compiler.read_decisions(src)

    def test_does_not_overwrite_reviewed_output(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "Система"}})
            dest = Path(root) / "approved.csv"
            dest.write_text("leave me alone", encoding="utf-8")
            with self.assertRaises(FileExistsError):
                compiler.compile_review(src, dest)
            self.assertEqual(dest.read_text(encoding="utf-8"), "leave me alone")

    def test_cli_never_needs_database_role_or_connects(self):
        with TemporaryDirectory() as root:
            src = self.review_file(root, [self.candidate()])
            self.edit(src, {0: {"decision": "approve", "approved_label": "Система"}})
            dest = Path(root) / "approved.csv"
            with mock.patch.object(compiler, "checked_csv", wraps=importer.checked_csv):
                with contextlib.redirect_stdout(io.StringIO()) as message:
                    result = compiler.main([str(src), "--output", str(dest)])
            self.assertEqual(result, 0)
            self.assertIn("no database changes", message.getvalue())
            self.assertEqual(importer.checked_csv(dest), [("tag", "Система", [])])


if __name__ == "__main__":
    unittest.main()
