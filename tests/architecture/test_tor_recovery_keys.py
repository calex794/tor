"""RECOVERY-04a: missing, replaced and mismatched historical key negatives."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_keys import (
    MASTER, V1, V2, Refused, bundle_fingerprints, capture, db_required_keys,
    read_private, verify,
)


def raw(value):
    return json.dumps(value, sort_keys=True).encode()


def content_key(byte):
    return base64.urlsafe_b64encode(bytes([byte]) * 32).decode().rstrip("=")


def bundle():
    return {"schema_version": 1, "active_kek_id": "content-kek-v1",
            "keys": {MASTER: base64.b64encode(b"m" * 32).decode(),
                     V1: content_key(1), V2: content_key(2)}}


def usage():
    return {"schema_version": 1, "storage_kek_ids": ["content-kek-v1", "content-kek-v2"],
            "admin_s3_profile_count": 1}


class RecoveryKeysTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.inventory = self.root / "inventory.json"
        self.db_usage = self.root / "db-usage.json"
        self.db_usage.write_bytes(raw(usage()))
        self.db_usage.chmod(0o600)
        capture(raw(bundle()), raw(usage()), self.inventory)
        self.inventory_raw = self.inventory.read_bytes()
        self.pin = hashlib.sha256(self.inventory_raw).hexdigest()

    def check(self, data=None, report=None, inventory=None, pin=None):
        return verify(raw(bundle() if data is None else data),
                      raw(usage() if report is None else report),
                      self.inventory_raw if inventory is None else inventory,
                      self.pin if pin is None else pin)

    def test_all_three_keys_and_active_id_pinned(self):
        self.assertEqual(self.check(), 3)
        self.assertEqual(self.inventory.stat().st_mode & 0o777, 0o600)
        for value in bundle()["keys"].values():
            self.assertNotIn(value.encode(), self.inventory_raw)
        with self.assertRaises(FileExistsError):
            capture(raw(bundle()), raw(usage()), self.inventory)

    def test_missing_or_newly_replaced_key_refused(self):
        for name in (MASTER, V1, V2):
            for missing in (True, False):
                data = bundle()
                if missing:
                    del data["keys"][name]
                else:
                    data["keys"][name] = (base64.b64encode(b"z" * 32).decode()
                                          if name == MASTER else content_key(9))
                with self.subTest(name=name, missing=missing), self.assertRaises(Refused):
                    self.check(data)

    def test_attacker_rewrites_bundle_and_inventory(self):
        data = bundle()
        data["keys"][V2] = content_key(9)
        forged = raw(bundle_fingerprints(raw(data)))
        with self.assertRaisesRegex(Refused, "inventory digest"):
            self.check(data=data, inventory=forged)

    def test_v1_required_even_when_database_only_references_v2(self):
        report = dict(usage(), storage_kek_ids=["content-kek-v2"])
        self.assertEqual(db_required_keys(raw(report)), {MASTER, V1, V2})
        data = bundle()
        del data["keys"][V2]
        out = self.root / "missing.json"
        with self.assertRaisesRegex(Refused, "database references"):
            capture(raw(data), raw(report), out)
        self.assertFalse(out.exists())

    def test_retained_v2_stays_required_even_when_active_v1(self):
        report = dict(usage(), storage_kek_ids=["content-kek-v1"])
        self.assertEqual(self.check(report=report), 3)
        data = bundle()
        del data["keys"][V2]
        with self.assertRaises(Refused):
            self.check(data=data, report=report)
        data = bundle()
        data["active_kek_id"] = "content-kek-v2"
        with self.assertRaisesRegex(Refused, "active KEK"):
            self.check(data=data)

    def test_unknown_schema_encoding_reuse_and_duplicate_rejected(self):
        for change in (
            lambda data: data["keys"].update(UNKNOWN=content_key(3)),
            lambda data: data["keys"].update({V1: data["keys"][V2]}),
            lambda data: data["keys"].update({V1: content_key(1) + "="}),
            lambda data: data["keys"].update({MASTER: "not-a-key"}),
            lambda data: data["keys"].update({V1: content_key(0)}),
            lambda data: data.update(schema_version=True),
            lambda data: data.update(active_kek_id=[]),
        ):
            data = bundle()
            change(data)
            with self.assertRaises(Refused):
                self.check(data)
        with self.assertRaises(Refused):
            bundle_fingerprints(b'{"schema_version":1,"schema_version":1}')
        for report in (dict(usage(), storage_kek_ids=["future-kek"]),
                       dict(usage(), admin_s3_profile_count=True)):
            with self.assertRaises(Refused):
                self.check(report=report)

    def test_master_required_when_encrypted_s3_profile_exists(self):
        data = bundle()
        del data["keys"][MASTER]
        report = dict(usage(), admin_s3_profile_count=0)
        out = self.root / "old-inventory.json"
        capture(raw(data), raw(report), out)
        inv = out.read_bytes()
        with self.assertRaisesRegex(Refused, "database references"):
            self.check(data=data, inventory=inv, pin=hashlib.sha256(inv).hexdigest())

    def test_private_evidence_permissions_and_links(self):
        self.assertEqual(read_private(self.inventory), self.inventory_raw)
        self.inventory.chmod(0o644)
        with self.assertRaises(Refused):
            read_private(self.inventory)
        self.inventory.chmod(0o600)
        linked = self.root / "hardlink"
        os.link(self.inventory, linked)
        with self.assertRaises(Refused):
            read_private(self.inventory)
        linked.unlink()
        linked.symlink_to(self.inventory)
        with self.assertRaises(OSError):
            read_private(linked)

    def test_cli_never_outputs_keys_or_claims_decryption(self):
        cmd = [sys.executable, str(ROOT / "tools/tor_recovery_keys.py"), "verify",
               "--db-usage", str(self.db_usage), "--inventory", str(self.inventory),
               "--expected-inventory-sha256", self.pin]
        for payload, code in ((raw(bundle()), 0), (b"invalid", 2), (b"x" * 65537, 2)):
            result = subprocess.run(cmd, input=payload, capture_output=True)
            self.assertEqual(result.returncode, code, result.stderr)
            for value in bundle()["keys"].values():
                self.assertNotIn(value.encode(), result.stdout + result.stderr)
            if code == 0:
                self.assertIn(b"decryption_tested=false recovery_accepted=false", result.stdout)


if __name__ == "__main__":
    unittest.main()
