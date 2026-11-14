"""Isolated regression tests: the manifest verifier never reads S3 or PostgreSQL."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "tools/i11_object_manifest.py"


class IndependentInventory(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        base = Path(self.tmp.name)
        self.source = base / "source"
        self.restore = base / "restore"
        self.source.mkdir()
        self.restore.mkdir()
        for root in (self.source, self.restore):
            (root / "private").mkdir()
            (root / "private" / "paid-v1.bin").write_bytes(b"ciphertext v1")
            (root / "public.bin").write_bytes(b"published")
        self.manifest = base / "independent-manifest.json"
        self.created = self.run_script("snapshot", "--root", str(self.source), "--output", str(self.manifest))
        self.assertEqual(self.created.returncode, 0, self.created.stderr)
        self.digest = hashlib.sha256(self.manifest.read_bytes()).hexdigest()

    @staticmethod
    def run_script(*args):
        return subprocess.run([sys.executable, str(SCRIPT), *args], text=True, capture_output=True)

    def check(self, *, digest=None):
        return self.run_script("verify", "--root", str(self.restore), "--manifest",
                               str(self.manifest), "--expected-manifest-sha256", digest or self.digest)

    def test_match(self):
        r = self.check()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("TOR_I11_OFFSITE_MANIFEST_VERIFIED objects=2", r.stdout)

    def test_deleted_private_object_refused(self):
        (self.restore / "private/paid-v1.bin").unlink()
        r = self.check()
        self.assertEqual(r.returncode, 2)
        self.assertIn("missing=1", r.stderr)
        self.assertNotIn("paid-v1.bin", r.stderr)

    def test_corrupted_same_size_refused(self):
        (self.restore / "private/paid-v1.bin").write_bytes(b"ciphertext v2")
        r = self.check()
        self.assertEqual(r.returncode, 2)
        self.assertIn("changed=1", r.stderr)

    def test_unexpected_object_refused(self):
        (self.restore / "extra.bin").write_bytes(b"extra")
        r = self.check()
        self.assertEqual(r.returncode, 2)
        self.assertIn("unexpected=1", r.stderr)

    def test_forged_manifest_checksum_refused(self):
        data = json.loads(self.manifest.read_text())
        data["objects"].pop()
        self.manifest.write_text(json.dumps(data))
        r = self.check()
        self.assertEqual(r.returncode, 2)
        self.assertIn("digest mismatch", r.stderr)

    def test_duplicate_unsorted_and_traversal_refused(self):
        original = json.loads(self.manifest.read_text())
        cases = []
        duplicate = json.loads(json.dumps(original))
        duplicate["objects"].append(duplicate["objects"][0])
        cases.append(duplicate)
        traversal = json.loads(json.dumps(original))
        traversal["objects"][0]["key"] = "../private/paid-v1.bin"
        cases.append(traversal)
        wrong_size = json.loads(json.dumps(original))
        wrong_size["objects"][0]["size"] = True
        cases.append(wrong_size)
        for malformed in cases:
            with self.subTest(malformed=malformed):
                self.manifest.write_text(json.dumps(malformed))
                r = self.check(digest=hashlib.sha256(self.manifest.read_bytes()).hexdigest())
                self.assertEqual(r.returncode, 2)

    def test_symlink_file_and_directory_refused(self):
        (self.restore / "public.bin").unlink()
        (self.restore / "public.bin").symlink_to(self.source / "public.bin")
        self.assertEqual(self.check().returncode, 2)
        (self.restore / "public.bin").unlink()
        (self.restore / "public.bin").write_bytes(b"published")
        (self.restore / "linked").symlink_to(self.source / "private", target_is_directory=True)
        self.assertEqual(self.check().returncode, 2)

    def test_duplicate_json_field_refused(self):
        original = self.manifest.read_text()
        malformed = original.replace('"schema_version":1', '"schema_version":1,"schema_version":1', 1)
        self.assertNotEqual(malformed, original)
        self.manifest.write_text(malformed)
        digest = hashlib.sha256(self.manifest.read_bytes()).hexdigest()
        r = self.check(digest=digest)
        self.assertEqual(r.returncode, 2)
        self.assertIn("duplicate manifest field", r.stderr)

    def test_manifest_0600_and_no_overwrite(self):
        import stat
        self.assertEqual(stat.S_IMODE(self.manifest.stat().st_mode), 0o600)
        original = self.manifest.read_bytes()
        r = self.run_script("snapshot", "--root", str(self.source), "--output", str(self.manifest))
        self.assertEqual(r.returncode, 2)
        self.assertEqual(self.manifest.read_bytes(), original)

    def test_empty_source_and_output_inside_tree_refused(self):
        empty = Path(self.tmp.name) / "empty"
        empty.mkdir()
        r = self.run_script("snapshot", "--root", str(empty), "--output", str(empty / "manifest.json"))
        self.assertEqual(r.returncode, 2)
        self.assertFalse((empty / "manifest.json").exists())
        r = self.run_script("snapshot", "--root", str(self.source), "--output", str(self.source / "manifest.json"))
        self.assertEqual(r.returncode, 2)


if __name__ == "__main__":
    unittest.main()
