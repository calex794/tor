import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

from drop_verify_restore_receipts import compare
from i11_object_manifest import Refused


class DropRestoreReceipts(unittest.TestCase):
    def test_exact_archive_and_manifest_required(self):
        with tempfile.TemporaryDirectory() as t:
            path = Path(t) / "retained.json"
            prefix = "private/free-packs/check/fixture"
            digest = "ab" * 32
            payload = {
                "schema_version": 1,
                "created_utc": "2026-11-17T00:00:00Z",
                "objects": [
                    {"key": prefix + ".manifest.json", "size": 11, "sha256": digest},
                    {"key": prefix + ".zst", "size": 27, "sha256": digest},
                ],
            }
            raw = json.dumps(payload).encode()
            path.write_bytes(raw)
            fingerprint = hashlib.sha256(raw).hexdigest()
            manifest = f"{prefix}.manifest.json,11,{digest}\n"
            pack = f"{prefix}.zst,27,{digest}\n"
            self.assertEqual(compare(path, fingerprint, io.StringIO(manifest + pack)), 2)
            with self.assertRaisesRegex(Refused, "undeclared=1"):
                compare(path, fingerprint, io.StringIO(pack))
            with self.assertRaisesRegex(Refused, "changed=1"):
                compare(path, fingerprint, io.StringIO(manifest + pack.replace(",27,", ",28,")))
            with self.assertRaisesRegex(Refused, "unbacked=1"):
                compare(path, fingerprint, io.StringIO(manifest + pack + f"{prefix}x.zst,27,{digest}\n"))
            with self.assertRaisesRegex(Refused, "duplicate or unsorted"):
                compare(path, fingerprint, io.StringIO(pack + manifest))
            with self.assertRaisesRegex(Refused, "digest mismatch"):
                compare(path, "00" * 32, io.StringIO(manifest + pack))
