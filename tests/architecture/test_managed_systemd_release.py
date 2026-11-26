"""DEPLOY-04: signed policy and injected systemd transaction failures."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import managed_systemd_release as unit
import release_bundle_v2 as bundle

SHA = "e" * 40
POLICY = (ROOT / bundle.SYSTEMD[unit.SOURCE]).read_bytes()


class FixtureSystemd:
    def __init__(self, directory, failure=None):
        self.directory = directory
        self.failure = failure
        self.calls = []
        self.baseline = {name: "unchanged" for name in unit.PROPERTIES}
        self.baseline.update(User="tor-deploy", Group="tor-deploy",
                             WorkingDirectory="/opt/tor", RestartUSec="3s")
        self.effective = self.baseline.copy()

    def verify(self):
        self.calls.append("verify")
        if self.failure == "verify" and (self.directory / unit.NAME).exists():
            raise RuntimeError("bad unit")

    def reload(self):
        self.calls.append("reload")
        path = self.directory / unit.NAME
        if path.exists() and self.failure == "reload":
            raise RuntimeError("failed daemon-reload")
        if not path.exists() and self.failure == "rollback":
            raise RuntimeError("failed rollback")
        self.effective = self.baseline.copy()
        if path.exists():
            self.effective.update(unit.policy(path.read_bytes()))
            if self.failure in ("override", "rollback"):
                self.effective["NoNewPrivileges"] = "no"
            if self.failure == "operator":
                self.effective["MemoryMax"] = "changed"

    def snapshot(self):
        self.calls.append("snapshot")
        return self.effective.copy()


class ManagedUnitTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        self.root = root
        self.directory = root / "tor-web.service.d"
        self.directory.mkdir()
        self.managed = unit.UnitPolicy(self.directory)
        self.operator = self.directory / "30-runtime.conf"
        self.operator.write_text("[Service]\nEnvironmentFile=/etc/tor/tor-runtime.env\n")
        self.original = self.operator.read_bytes()
        repo = root / "repo"
        binaries = root / "binaries"
        binaries.mkdir()
        for name in bundle.BINARIES:
            (binaries / name).write_text("fixture " + name)
        for name, rel in bundle.CONFIGS.items():
            p = repo / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(POLICY if name == unit.SOURCE else b"fixture")
        self.source = root / "bundle"
        bundle.build(repo, binaries, self.source, SHA)
        key = root / "key"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(key))
        self.signers = root / "allowed_signers"
        self.signers.write_text(bundle.IDENTITY + " " + key.with_suffix(".pub").read_text())
        self.ssh("-Y", "sign", "-f", str(key), "-n", bundle.NAMESPACE,
                 str(self.source / bundle.MANIFEST))

    def ssh(self, *args):
        subprocess.run(["ssh-keygen", *args], check=True, timeout=15,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def apply(self, systemd):
        return self.managed.apply(self.source, SHA, self.signers, systemd)

    def test_apply_and_restore_preserve_operator_file(self):
        systemd = FixtureSystemd(self.directory)
        receipt = self.apply(systemd)
        self.assertEqual((self.directory / unit.NAME).read_bytes(), POLICY)
        self.assertEqual((self.directory / unit.NAME).stat().st_mode & 0o777, 0o644)
        self.assertEqual(self.operator.read_bytes(), self.original)
        self.managed.restore(receipt, systemd)
        self.assertFalse((self.directory / unit.NAME).exists())
        self.assertEqual(systemd.snapshot(), receipt.baseline)
        self.assertEqual(self.operator.read_bytes(), self.original)

    def test_candidate_failures_restore_previous_effective_policy(self):
        for failure in ("verify", "reload", "override", "operator"):
            with self.subTest(failure=failure):
                systemd = FixtureSystemd(self.directory, failure)
                with self.assertRaisesRegex(unit.UnitError, "UNIT_ROLLBACK_VERIFIED"):
                    self.apply(systemd)
                self.assertFalse((self.directory / unit.NAME).exists())
                self.assertEqual(systemd.effective, systemd.baseline)
                self.assertEqual(self.operator.read_bytes(), self.original)

    def test_rollback_failure_is_distinct(self):
        with self.assertRaisesRegex(unit.UnitError, "UNIT_ROLLBACK_UNVERIFIED"):
            self.apply(FixtureSystemd(self.directory, "rollback"))

    def test_prepared_receipt_rejects_operator_drift(self):
        systemd = FixtureSystemd(self.directory)
        receipt = self.managed.plan(self.source, SHA, self.signers, systemd)
        systemd.effective["MemoryMax"] = "operator modified"
        with self.assertRaisesRegex(unit.UnitError, "changed after preflight"):
            self.managed.apply_receipt(receipt, systemd)
        self.assertFalse((self.directory / unit.NAME).exists())

    def test_same_policy_retry_no_reload(self):
        systemd = FixtureSystemd(self.directory)
        self.apply(systemd)
        systemd.calls.clear()
        self.apply(systemd)
        self.assertEqual(systemd.calls, ["verify", "snapshot"])

    def test_restore_previous_managed_policy(self):
        path = self.directory / unit.NAME
        previous = POLICY.replace(b"RestartSec=2s", b"RestartSec=5s")
        path.write_bytes(previous)
        path.chmod(0o644)
        systemd = FixtureSystemd(self.directory)
        systemd.reload()
        receipt = self.apply(systemd)
        self.managed.restore(receipt, systemd)
        self.assertEqual(path.read_bytes(), previous)

    def test_unknown_existing_file_is_not_overwritten(self):
        path = self.directory / unit.NAME
        path.write_text("[Service]\nEnvironment=OPERATOR=1\n")
        path.chmod(0o644)
        systemd = FixtureSystemd(self.directory)
        with self.assertRaises(unit.UnitError):
            self.apply(systemd)
        self.assertEqual(systemd.calls, [])
        self.assertIn("OPERATOR", path.read_text())

    def test_signed_policy_cannot_add_arbitrary_directives(self):
        for line in (b"ExecStart=/bin/sh\n", b"User=root\n",
                     b"EnvironmentFile=/etc/shadow\n", b"RestartSec=0s\n"):
            with self.subTest(line=line), self.assertRaises(unit.UnitError):
                unit.policy(POLICY + line)
        with self.assertRaises(unit.UnitError):
            unit.policy(POLICY.replace(b"NoNewPrivileges=yes", b"NoNewPrivileges=no"))

    def test_modified_signed_policy_is_refused(self):
        (self.source / unit.SOURCE).write_bytes(POLICY.replace(b"2s", b"3s"))
        systemd = FixtureSystemd(self.directory)
        with self.assertRaises(bundle.BundleError):
            self.apply(systemd)
        self.assertEqual(systemd.calls, [])

    def test_wrong_service_identity_refused(self):
        systemd = FixtureSystemd(self.directory)
        systemd.effective["User"] = "root"
        with self.assertRaises(unit.UnitError):
            self.apply(systemd)
        self.assertFalse((self.directory / unit.NAME).exists())

    def test_link_at_managed_path_refused(self):
        path = self.directory / unit.NAME
        path.symlink_to(self.operator)
        systemd = FixtureSystemd(self.directory)
        with self.assertRaises(trusted_error()):
            self.apply(systemd)
        self.assertEqual(self.operator.read_bytes(), self.original)


def trusted_error():
    return unit.trusted.EdgeError


if __name__ == "__main__":
    unittest.main()
