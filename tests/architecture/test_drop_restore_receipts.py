import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))

from drop_verify_restore_receipts import compare, compare_restored_pack
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

    def test_selected_pack_requires_both_exact_restored_objects(self):
        with tempfile.TemporaryDirectory() as t:
            work = Path(t)
            root = work / "restored"
            stem = "private/free-packs/check/fixture"
            sidecar, pack = stem + ".manifest.json", stem + ".zst"
            (root / stem).parent.mkdir(parents=True)
            payloads = {sidecar: b"manifest bytes", pack: b"stored PACK bytes" * 3}
            for key, content in payloads.items():
                (root / key).write_bytes(content)
            entries = [
                {"key": key, "size": len(content), "sha256": hashlib.sha256(content).hexdigest()}
                for key, content in payloads.items()
            ]
            unrelated = dict(entries[1], key=stem + "-unrelated.zst")
            manifest = work / "retained.json"

            def write_manifest(objects):
                raw = json.dumps({
                    "schema_version": 1,
                    "created_utc": "2026-11-17T00:00:00Z",
                    "objects": sorted(objects, key=lambda obj: obj["key"]),
                }).encode()
                manifest.write_bytes(raw)
                return hashlib.sha256(raw).hexdigest()

            fingerprint = write_manifest([*entries, unrelated])
            lines = [f"{row['key']},{row['size']},{row['sha256']}\n" for row in entries]
            receipts = "".join(lines)

            def check(report=receipts, digest=fingerprint):
                return compare_restored_pack(manifest, digest, io.StringIO(report), root)

            self.assertEqual(check(), 2)
            with self.assertRaisesRegex(Refused, "expected exactly two"):
                check(lines[1])
            with self.assertRaisesRegex(Refused, "expected exactly two"):
                check(receipts + f"{stem}a.zst,27,{'cd' * 32}\n")
            with self.assertRaisesRegex(Refused, "invalid PACK or manifest pair"):
                check(receipts.replace(sidecar, stem + "-wrong.manifest.json"))
            with self.assertRaisesRegex(Refused, "digest mismatch"):
                check(digest="cd" * 32)
            without_sidecar = write_manifest([entries[1], unrelated])
            with self.assertRaisesRegex(Refused, "differ from the retained manifest"):
                check(digest=without_sidecar)
            self.assertEqual(write_manifest([*entries, unrelated]), fingerprint)

            (root / pack).unlink()
            with self.assertRaisesRegex(Refused, "restored PACK or manifest bytes mismatch"):
                check()
            (root / pack).write_bytes(b"x" * len(payloads[pack]))
            with self.assertRaisesRegex(Refused, "restored PACK or manifest bytes mismatch"):
                check()
            (root / pack).write_bytes(b"wrong size")
            with self.assertRaisesRegex(Refused, "length mismatch"):
                check()
            (root / pack).write_bytes(payloads[pack])
            extra = root / unrelated["key"]
            extra.write_bytes(payloads[pack])
            with self.assertRaisesRegex(Refused, "unexpected object in restored tree"):
                check()
            extra.unlink()
            (root / pack).unlink()
            (root / pack).symlink_to(root / sidecar)
            with self.assertRaisesRegex(Refused, "non-regular object refused"):
                check()
