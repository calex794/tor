"""DROP-08b offline negative controls for archived FREE references."""
from pathlib import Path
import io
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
from drop08_verify_historical_free_pins import Refused, verify


class HistoricalFreePins(unittest.TestCase):
    GOOD = "layer,11,1,1,0,0,1\nsaved,12,2,1,0,1,0\n"

    def test_registered_pack_and_single(self):
        self.assertEqual(verify(io.StringIO(self.GOOD)), {"layer": 1, "saved": 1})

    def test_inline_legacy_pg_text(self):
        s = "layer,11,1,1,1,0,0\nsaved,12,2,1,0,1,0\n"
        self.assertEqual(verify(io.StringIO(s)), {"layer": 1, "saved": 1})

    def test_export_requires_active_checksum_matched_pack(self):
        sql = (Path(__file__).resolve().parents[2] / "tools/drop08_export_historical_free_pins.sql").read_text()
        self.assertIn("FROM free_edition_active_storage a", sql)
        self.assertIn("AND g.generation=a.generation AND g.placement_kind='pack'", sql)
        self.assertIn("AND g.content_sha256=v.content_sha256", sql)
        self.assertIn("AND s.sha256=v.content_sha256", sql)
        self.assertIn("AND a.version_id=p.version_id", sql)
        self.assertIn("AND c.access_level='free' AND c.price_amount=0", sql)

    def test_missing_receipt_refused(self):
        with self.assertRaisesRegex(Refused, "missing_source_receipts=1"):
            verify(io.StringIO("layer,11,1,1,0,0,0\nsaved,12,2,1,0,1,0\n"))

    def test_wrong_layer_book_or_version_membership_refused(self):
        with self.assertRaisesRegex(Refused, "invalid historical chapter-version or Layer book membership"):
            verify(io.StringIO("layer,11,1,0,0,0,1\nsaved,12,2,1,0,1,0\n"))

    def test_missing_either_kind_is_not_complete(self):
        for text in ("layer,11,1,1,0,0,1\n", "saved,12,2,1,0,1,0\n", ""):
            with self.subTest(text=text), self.assertRaisesRegex(Refused, "exercised"):
                verify(io.StringIO(text))

    def test_corruption_unsorted_duplicate_and_false_success(self):
        cases = [
            ("layer,11,1,1,0,0,1\nlayer,11,1,1,0,0,1\nsaved,12,2,1,0,1,0\n", "duplicate"),
            ("saved,12,2,1,0,1,0\nlayer,11,1,1,0,0,1\n", "unsorted"),
            ("layer,011,1,1,0,0,1\nsaved,12,2,1,0,1,0\n", "identity"),
            ("layer,11,1,1,0,0,yes\nsaved,12,2,1,0,1,0\n", "evidence"),
            ("layer,11,1,1,0,0,1,unexpected\nsaved,12,2,1,0,1,0\n", "oversized"),
        ]
        for content, reason in cases:
            with self.subTest(reason=reason), self.assertRaisesRegex(Refused, reason):
                verify(io.StringIO(content))


if __name__ == "__main__":
    unittest.main()
