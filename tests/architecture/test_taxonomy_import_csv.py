"""Offline curator CSV importer: preflight validation without PostgreSQL."""
import os
from pathlib import Path
import sys
import tempfile
import unittest

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
