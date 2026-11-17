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
