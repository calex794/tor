"""Explicit interface admission and verified return to the local vhost."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import admit_clean_install as admission

LOCAL = (ROOT / "ops/install/pandaolate.conf").read_bytes()
IP = "192.0.2.10"
CANDIDATE = admission.render(LOCAL, IP)


class Ops:
    def __init__(self, journal, failure=None):
        self.journal, self.failure = journal, failure
        self.data = self.live = LOCAL

    def current(self):
        return self.data

    def replace(self, expected, data):
        assert self.data == expected
        assert self.journal.load() in ("active", "rollback_pending")
        self.data = data

    def reload(self):
        if self.failure == "reload" and self.data == CANDIDATE:
            raise RuntimeError("reload")
        self.live = self.data

    def preflight_public(self):
        if self.failure == "preflight":
            raise RuntimeError("preflight")

    def verify_local(self):
        assert self.live == LOCAL
        if self.failure == "rollback" and self.journal.load() == "rollback_pending":
            raise RuntimeError("rollback")

    def verify_public(self):
        assert self.live == CANDIDATE
        if self.failure in ("probe", "rollback"):
            raise RuntimeError("probe")


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.path = Path(temp.name) / "journal.json"
        self.journal = admission.Journal(self.path, {"format": 1, "ipv4": IP})

    def test_render_only_adds_four_bindings(self):
        lines = CANDIDATE.splitlines(keepends=True)
        added = [line for line in lines if b"listen " + IP.encode() in line]
        self.assertEqual(len(added), 4)
        self.assertEqual(b"".join(line for line in lines if line not in added), LOCAL)
        self.assertIn(b"deny all;", CANDIDATE)

    def test_invalid_addresses_refused(self):
        for address in ("127.0.0.1", "0.0.0.0", "::1", "224.0.0.1", "169.254.1.1",
                        "192.0.2.10; return 200"):
            with self.subTest(address=address), self.assertRaises(ValueError):
                admission.render(LOCAL, address)

    def test_exact_assigned_address(self):
        info = [{"addr_info": [{"family": "inet", "scope": "global", "local": IP}]}]
        admission.require_assigned(IP, json.dumps(info))
        for other in ([], info + info):
            with self.assertRaises(admission.Refused):
                admission.require_assigned(IP, json.dumps(other))

    def test_listener_set_rejects_wildcard_and_public_pg(self):
        local = "".join(f"LISTEN 0 64 127.0.0.1:{p} 0.0.0.0:*\n"
                        for p in (80, 443, 3000, 5432))
        public = local + "".join(f"LISTEN 0 64 {IP}:{p} 0.0.0.0:*\n" for p in (80, 443))
        admission.check_listeners(local)
        admission.check_listeners(public, IP)
        for text in (public + f"LISTEN 0 64 {IP}:5432 0.0.0.0:*\n",
                     public.replace(f"{IP}:443", "0.0.0.0:443"), local):
            with self.assertRaises(admission.Refused):
                admission.check_listeners(text, IP)

    def test_admit_idempotent_and_restore(self):
        ops = Ops(self.journal)
        self.assertEqual(admission.admit(self.journal, ops, LOCAL, CANDIDATE), "admitted")
        self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(admission.admit(self.journal, ops, LOCAL, CANDIDATE), "admitted")
        self.assertEqual(admission.admit(self.journal, ops, LOCAL, CANDIDATE, restore=True), "rolled_back")
        self.assertEqual((ops.data, ops.live), (LOCAL, LOCAL))

    def test_reload_and_probe_failures_restore(self):
        for failure in ("reload", "probe"):
            ops = Ops(self.journal, failure)
            with self.assertRaisesRegex(admission.Refused, "ADMISSION_ROLLBACK_VERIFIED"):
                admission.admit(self.journal, ops, LOCAL, CANDIDATE)
            self.assertEqual(self.journal.load(), "rolled_back")
            self.assertEqual((ops.data, ops.live), (LOCAL, LOCAL))

    def test_interrupted_admission_requires_explicit_restore(self):
        ops = Ops(self.journal)
        self.journal.save("active")
        ops.data = CANDIDATE
        with self.assertRaisesRegex(admission.Refused, "explicit restore-local"):
            admission.admit(self.journal, ops, LOCAL, CANDIDATE)
        admission.admit(self.journal, ops, LOCAL, CANDIDATE, restore=True)
        self.assertEqual((ops.data, ops.live), (LOCAL, LOCAL))

    def test_unverified_rollback_preserved(self):
        ops = Ops(self.journal, "rollback")
        with self.assertRaisesRegex(admission.Refused, "ADMISSION_ROLLBACK_UNVERIFIED"):
            admission.admit(self.journal, ops, LOCAL, CANDIDATE)
        self.assertEqual(self.journal.load(), "rollback_pending")
        ops.failure = None
        admission.admit(self.journal, ops, LOCAL, CANDIDATE, restore=True)

    def test_unrelated_config_not_overwritten(self):
        ops = Ops(self.journal)
        ops.data = b"operator edit"
        self.journal.save("active")
        with self.assertRaisesRegex(admission.Refused, "outside transaction"):
            admission.admit(self.journal, ops, LOCAL, CANDIDATE, restore=True)
        self.assertEqual(ops.data, b"operator edit")


if __name__ == "__main__":
    unittest.main()
