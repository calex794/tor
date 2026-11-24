"""RECOVERY-04b: exact selection, existing manifest verifier and safe command."""
from argparse import Namespace
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_objects import (
    Refused, approved_objects, command, prepare_plan, restore, validate_config,
    verify_staged,
)

ENDPOINT = "https://backup.example.invalid"
ARCHIVE = "tor-backup/dr"
CONFIG = b"""[tor-offsite-read]
type = crypt
remote = tor-offsite-s3:tor-backup/dr
password = synthetic-only
[tor-offsite-s3]
type = s3
provider = Other
env_auth = false
access_key_id = fixture-access
secret_access_key = fixture-secret
endpoint = https://backup.example.invalid
region = test-region
"""
FILES = {
    "private/free-packs/abc.manifest.json": b"fixture-manifest",
    "private/free-packs/abc.zst": b"fixture-pack",
    "private/paid/historical-v1": b"fixture-ciphertext",
}


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def manifest(files):
    entries = [{"key": name, "size": len(data), "sha256": digest(data)}
               for name, data in sorted(files.items())]
    return json.dumps({"schema_version": 1, "created_utc": "2026-01-01T00:00:00Z",
                       "objects": entries}).encode()


class RecoveryObjectsTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name).resolve()
        self.root.chmod(0o710)
        self.objects = self.root / "objects"
        self.objects.mkdir(mode=0o700)
        self.evidence = self.root / "evidence"
        self.evidence.mkdir(mode=0o700)
        self.raw = manifest(FILES)
        for name, raw in (("objects-rclone.conf", CONFIG), ("objects-manifest.json", self.raw)):
            path = self.evidence / name
            path.write_bytes(raw)
            path.chmod(0o600)
        self.args = Namespace(expected_manifest_sha256=digest(self.raw),
                              expected_config_sha256=digest(CONFIG), endpoint=ENDPOINT,
                              archive_root=ARCHIVE, operator_confirm_read_only_repository=True)
        self.entries = approved_objects(self.raw, digest(self.raw))

    def stage(self, files):
        for key, value in files.items():
            path = self.objects / key
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(value)

    def test_plan_is_local_read_only(self):
        before = set(self.evidence.iterdir())
        with patch("tor_recovery_objects.subprocess.run") as run:
            entries, total = prepare_plan(self.args, self.root)
            run.assert_not_called()
        self.assertEqual(entries, self.entries)
        self.assertEqual(total, sum(map(len, FILES.values())))
        self.assertEqual(set(self.evidence.iterdir()), before)
        self.assertEqual(list(self.objects.iterdir()), [])

    def test_manifest_pin_and_path_collision(self):
        with self.assertRaises(Refused):
            approved_objects(self.raw, "0" * 64)
        for names in ({"a": b"a", "a/b": b"b"}, {"../a": b"a"},
                      {"private/name\nother": b"a"}, {"private/with space": b"a"}):
            data = manifest(names)
            with self.assertRaises(Refused):
                approved_objects(data, digest(data))

    def test_config_only_independent_pinned_archive(self):
        validate_config(CONFIG, digest(CONFIG), ENDPOINT, ARCHIVE)
        with self.assertRaises(Refused):
            validate_config(CONFIG, "a" * 64, ENDPOINT, ARCHIVE)
        for data, endpoint, archive in (
            (CONFIG.replace(b"tor-backup/dr", b"tore/dr"), ENDPOINT, "tore/dr"),
            (CONFIG.replace(b"env_auth = false", b"env_auth = true"), ENDPOINT, ARCHIVE),
            (CONFIG.replace(b"type = s3", b"type = local"), ENDPOINT, ARCHIVE),
            (CONFIG + b"no_verify_ssl = true\n", ENDPOINT, ARCHIVE),
            (CONFIG + b"\n[unexpected]\ntype = s3\n", ENDPOINT, ARCHIVE),
            (CONFIG, "http://backup.example.invalid", ARCHIVE),
            (CONFIG, ENDPOINT, "tor-backup/other"),
        ):
            with self.assertRaises(Refused):
                validate_config(data, digest(data), endpoint, archive)

    def test_existing_files_or_prior_attempt_are_not_overwritten(self):
        self.stage({"existing": b"preserve"})
        with self.assertRaises(Refused):
            prepare_plan(self.args, self.root)
        self.assertEqual((self.objects / "existing").read_bytes(), b"preserve")
        (self.objects / "existing").unlink()
        (self.evidence / "objects-attempt").write_bytes(b"started")
        with self.assertRaises(Refused):
            prepare_plan(self.args, self.root)

    def test_actual_staged_bytes_and_exact_set_verified(self):
        self.stage(FILES)
        self.assertEqual(verify_staged(self.objects, self.entries), 3)
        path = self.objects / "private/paid/historical-v1"
        path.write_bytes(b"x" * len(FILES["private/paid/historical-v1"]))
        with self.assertRaises(Refused):
            verify_staged(self.objects, self.entries)
        path.unlink()
        with self.assertRaises(Refused):
            verify_staged(self.objects, self.entries)
        self.stage(FILES)
        self.stage({"extra": b"unexpected"})
        with self.assertRaises(Refused):
            verify_staged(self.objects, self.entries)

    def test_restore_command_only_selected_read_from_offsite(self):
        cmd = command(self.root)
        self.assertIn("tor-offsite-read:objects", cmd)
        self.assertIn("--files-from-raw", cmd)
        self.assertIn("--immutable", cmd)
        self.assertEqual(cmd[cmd.index("copy") + 2], str(self.objects))
        for forbidden in ("sync", "move", "delete", "purge", "--delete-before"):
            self.assertNotIn(forbidden, cmd)
        self.assertNotIn("fixture-secret", " ".join(cmd))
        with patch("tor_recovery_objects.os.access", return_value=True), \
                patch("tor_recovery_objects.subprocess.run") as run:
            self.args.operator_confirm_read_only_repository = False
            with self.assertRaises(Refused):
                restore(self.args, self.root, self.entries)
            run.assert_not_called()
            self.args.operator_confirm_read_only_repository = True

            def copied(*args, **kwargs):
                self.stage(FILES)
                self.assertEqual(kwargs["env"]["HOME"], str(self.evidence))
                self.assertNotIn("AWS_SECRET_ACCESS_KEY", kwargs["env"])
                return Namespace(returncode=0)

            run.side_effect = copied
            restore(self.args, self.root, self.entries)
            self.assertEqual((self.evidence / "objects-selection.txt").read_text(),
                             "".join(name + "\n" for name in sorted(FILES)))
            run.assert_called_once()

    def test_successful_download_command_cannot_hide_missing_object(self):
        with patch("tor_recovery_objects.os.access", return_value=True), \
                patch("tor_recovery_objects.subprocess.run", return_value=Namespace(returncode=0)):
            self.stage({"unrelated": b"x"})
            # Simulate a downloader reporting success with only some selected bytes.
            (self.objects / "unrelated").unlink()
            self.stage(dict(list(FILES.items())[:2]))
            with self.assertRaises(Refused):
                restore(self.args, self.root, self.entries)
            self.assertTrue((self.evidence / "objects-attempt").exists())

    def test_private_evidence_symlink_refused(self):
        path = self.evidence / "objects-manifest.json"
        moved = self.evidence / "other"
        path.rename(moved)
        path.symlink_to(moved)
        with self.assertRaises(OSError):
            prepare_plan(self.args, self.root)


if __name__ == "__main__":
    unittest.main()
