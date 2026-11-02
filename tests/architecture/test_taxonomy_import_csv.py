"""Offline curator CSV importer: preflight validation without PostgreSQL."""
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
import taxonomy_import_csv as importer


class TaxonomyImportTests(unittest.TestCase):
    def csv(self, content):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        path = Path(temp.name) / "reviewed.csv"
        path.write_text(content, encoding="utf-8")
        return str(path)

    def test_dry_run_utf8_and_aliases_without_db(self):
        path = self.csv("kind,label,aliases\n"
                        "tag,Система,System|系统\n"
                        "genre,Фэнтези,Fantasy\n")
        rows = importer.checked_csv(path)
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0][2], ["System", "系统"])
        old = os.environ.pop("TOR_TAXONOMY_ADMIN_URL", None)
        try:
            self.assertEqual(importer.main([path, "--curator", "ci-review"]), 0)
        finally:
            if old is not None:
                os.environ["TOR_TAXONOMY_ADMIN_URL"] = old

    def test_conflicting_alias_and_canonical_rejected(self):
        file = self.csv("kind,label,aliases\n"
                        "tag,Система,System\n"
                        "tag,System,\n")
        with self.assertRaisesRegex(ValueError, "colliding"):
            importer.checked_csv(file)

    def test_unicode_yo_case_and_space_collisions(self):
        # Mirrors current database key contract, not full Unicode NFKC.
        for a, b in [
            ("Ёжик", "Ежик"),
            ("СИСТЕМА", "система"),
            ("Космическая  опера", "Космическая опера"),
        ]:
            with self.subTest(a=a, b=b):
                file = self.csv(
                    "kind,label,aliases\n"
                    f"tag,{a},\n"
                    f"tag,{b},\n"
                )
                with self.assertRaisesRegex(ValueError, "colliding"):
                    importer.checked_csv(file)

    def test_nfkc_compatibility_collisions(self):
        for plain, compatible in [
            ("Fantasy", "Ｆａｎｔａｓｙ"),
            ("System", "Ｓｙｓｔｅｍ"),
            ("Café", "Cafe\u0301"),
        ]:
            with self.subTest(plain=plain):
                path = self.csv(
                    "kind,label,aliases\n"
                    f"tag,{plain},\n"
                    f"tag,{compatible},\n"
                )
                with self.assertRaisesRegex(ValueError, "colliding"):
                    importer.checked_csv(path)

    def test_nfkc_does_not_rewrite_display_label(self):
        rows = importer.checked_csv(self.csv(
            "kind,label,aliases\n"
            "genre,Ｓｙｓｔｅｍ,ＳｙｓｔｅｍⅣ\n"
        ))
        self.assertEqual(rows, [("genre", "Ｓｙｓｔｅｍ", ["ＳｙｓｔｅｍⅣ"])])
        self.assertEqual(importer.key("ＳｙｓｔｅｍⅣ"), "systemiv")

    def test_starter_proposal_is_reviewable_not_a_claimed_newboo_export(self):
        # This is explicitly a human-review *proposal*, not approved source data.
        source = (
            Path(__file__).resolve().parents[2]
            / "data" / "taxonomy" / "starter-proposal-v1.csv"
        )
        self.assertIn("proposal", source.name)
        rows = importer.checked_csv(source)
        self.assertEqual(len(rows), 144)
        self.assertEqual(sum(kind == "genre" for kind, _, _ in rows), 34)
        self.assertEqual(sum(kind == "tag" for kind, _, _ in rows), 110)
        self.assertEqual(sum(len(aliases) for _, _, aliases in rows), 151)
        self.assertFalse(any(kind == "fandom" for kind, _, _ in rows))
        self.assertIn(("tag", "Система", ["System", "系统"]), rows)
        self.assertIn(("genre", "Сянься", ["Xianxia", "仙侠"]), rows)
        self.assertIn(("genre", "Фэнтези", []), rows)

    def test_starter_proposal_preflight_never_requires_database(self):
        source = (
            Path(__file__).resolve().parents[2]
            / "data" / "taxonomy" / "starter-proposal-v1.csv"
        )
        # Review-only is the default, no credentials or database connection.
        existing = os.environ.pop("TOR_TAXONOMY_ADMIN_URL", None)
        try:
            self.assertEqual(importer.main(
                [str(source), "--curator", "ci-starter-review"]
            ), 0)
        finally:
            if existing is not None:
                os.environ["TOR_TAXONOMY_ADMIN_URL"] = existing

    def test_read_only_audit_uses_database_keys_and_public_facets(self):
        self.assertIn("BEGIN TRANSACTION READ ONLY;", importer.AUDIT_SQL)
        self.assertIn("FROM public_catalog_facets", importer.AUDIT_SQL)
        self.assertNotIn("FROM books", importer.AUDIT_SQL)
        self.assertIn("book_taxonomy_key(", importer.AUDIT_SQL)
        self.assertIn("jsonb_array_elements_text(c.aliases)", importer.AUDIT_SQL)
        file = self.csv("kind,label,aliases\ntag,Система,System|系统\n")
        result = {
            "row": 1, "kind": "tag", "label": "Система",
            "action": "approve_candidate", "term_id": 23,
            "db_status": "candidate", "public_uses": 4,
            "public_spellings": 2, "alias_conflicts": [],
            "can_apply": True,
        }
        url = "postgresql://tor_migrator:ci@127.0.0.1:5432/tor_ci"
        with mock.patch.dict(os.environ, {"TOR_TAXONOMY_ADMIN_URL": url}):
            with mock.patch.object(
                importer, "run_sql", return_value=json.dumps(result)
            ) as sql:
                with contextlib.redirect_stdout(io.StringIO()) as stdout:
                    with contextlib.redirect_stderr(io.StringIO()) as stderr:
                        status = importer.main([
                            file, "--curator", "ci-review", "--audit-db"
                        ])
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(stdout.getvalue().strip()), result)
        self.assertIn("0 conflicts", stderr.getvalue())
        self.assertEqual(sql.call_count, 1)
        args, kwargs = sql.call_args
        self.assertEqual(args, (url, importer.AUDIT_SQL))
        self.assertEqual(json.loads(kwargs["records"]), [{
            "ordinal": 1, "kind": "tag", "label": "Система",
            "aliases": ["System", "系统"],
        }])

    def test_audit_reports_conflicts_without_writing(self):
        file = self.csv("kind,label,aliases\ntag,Система,System\n")
        url = "postgresql://tor_migrator:ci@127.0.0.1:5432/tor_ci"
        result = {
            "row": 1, "action": "create", "can_apply": False,
            "alias_conflicts": [{"alias": "System", "canonical_id": 17}],
        }
        with mock.patch.dict(os.environ, {"TOR_TAXONOMY_ADMIN_URL": url}):
            with mock.patch.object(importer, "run_sql", return_value=json.dumps(result)):
                with contextlib.redirect_stdout(io.StringIO()) as stdout:
                    with contextlib.redirect_stderr(io.StringIO()) as stderr:
                        status = importer.main([
                            file, "--curator", "ci-review", "--audit-db"
                        ])
        self.assertEqual(status, 2)
        self.assertIn("1 conflicts", stderr.getvalue())
        self.assertEqual(json.loads(stdout.getvalue())["alias_conflicts"][0]["canonical_id"], 17)

    def test_audit_cannot_run_with_write_flags(self):
        file = self.csv("kind,label,aliases\ntag,Система,\n")
        url = "postgresql://tor_migrator:ci@127.0.0.1:5432/tor_ci"
        with mock.patch.dict(os.environ, {"TOR_TAXONOMY_ADMIN_URL": url}):
            with mock.patch.object(importer, "run_sql") as sql:
                for flags in (["--apply"], ["--confirm-db", "tor_ci"]):
                    with self.subTest(flags=flags):
                        with contextlib.redirect_stderr(io.StringIO()):
                            status = importer.main([
                                file, "--curator", "ci-review", "--audit-db",
                                *flags,
                            ])
                        self.assertEqual(status, 2)
                sql.assert_not_called()

    def test_audit_rejects_missing_records_instead_of_false_pass(self):
        rows = [("tag", f"Tag{i}", []) for i in range(51)]
        url = "postgresql://tor_migrator:ci@127.0.0.1:5432/tor_ci"
        with mock.patch.object(importer, "run_sql", return_value='{"row": 1}'):
            with self.assertRaisesRegex(RuntimeError, "incomplete"):
                importer.audit_terms(rows, url)

    def test_quote_is_data_not_executable_sql(self):
        transaction = importer.make_transaction(
            [("fandom", "Wizard's Tale", ["魔法师"])], "ci-review"
        )
        self.assertIn("BEGIN;", transaction)
        self.assertIn("COMMIT;", transaction)
        self.assertIn("Wizard''s Tale", transaction)
        self.assertIn("admin_taxonomy_add_alias", transaction)

    def test_malformed_rows_and_injection_blocked(self):
        for content in [
            "kind,label,aliases\ntag, Leading,",
            "kind,label,aliases\ntag,Bad\\\\Slash,",
            "kind,label,aliases\ntag,\"Bad\\nInjected\",",
            "kind,label\ntag,NoAliases",
        ]:
            with self.subTest(content=content):
                with self.assertRaises(ValueError):
                    importer.checked_csv(self.csv(content))

    def test_batch_needs_auditor_label(self):
        with self.assertRaises(ValueError):
            importer.make_transaction([("tag", "Good", [])], "invalid actor")


if __name__ == "__main__":
    unittest.main()
