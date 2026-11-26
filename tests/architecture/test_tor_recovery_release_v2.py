"""RECOVERY-01 with v2: actual signed release, independent pins, inert stage."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import release_bundle_v2 as bundle
import tor_recovery_release as recovery

SHA = "a" * 40


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class RecoveryV2Tests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.key = self.root / "key"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(self.key))
        self.signers = self.root / "signers"
        self.signers.write_text(bundle.IDENTITY + " " + self.key.with_suffix(".pub").read_text())
        self.signer_pin = digest(self.signers)
        binaries = self.root / "binaries"
        binaries.mkdir()
        for name in bundle.BINARIES:
            (binaries / name).write_text("inert fixture " + name)
        self.source = self.root / "source"
        bundle.build(ROOT, binaries, self.source, SHA)
        self.sign()
        self.pin = digest(self.source / bundle.MANIFEST)
        self.stage = self.root / "stage"
        self.stage.mkdir(mode=0o700)
        self.destination = self.stage / ("tor-recovery-" + SHA)

    def ssh(self, *args):
        subprocess.run(["/usr/bin/ssh-keygen", *args], check=True,
                       capture_output=True, timeout=15)

    def sign(self):
        (self.source / bundle.SIGNATURE).unlink(missing_ok=True)
        self.ssh("-Y", "sign", "-f", str(self.key), "-n", bundle.NAMESPACE,
                 str(self.source / bundle.MANIFEST))

    def verify(self, **changes):
        options = dict(bundle=self.source, commit=SHA, manifest_pin=self.pin,
                       signers_file=self.signers, signer_pin=self.signer_pin,
                       stage_root=self.stage)
        options.update(changes)
        return recovery.verify_release_v2(**options)

    def test_read_only_and_atomic_inert_stage(self):
        self.verify(stage_root=None)
        self.assertFalse(self.destination.exists())
        self.verify()
        self.assertEqual(self.destination.stat().st_mode & 0o777, 0o700)
        self.assertEqual({p.name for p in self.destination.iterdir()}, bundle.FILES)
        for name in bundle.FILES:
            path = self.destination / name
            self.assertEqual(path.read_bytes(), (self.source / name).read_bytes())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.verify(bundle=self.destination, stage_root=None)

    def test_replacement_signed_by_same_key_refused_by_old_manifest_pin(self):
        (self.source / "nginx-http.conf").write_bytes(b"replacement")
        hashes = {name: digest(self.source / name) for name in bundle.PAYLOAD}
        (self.source / bundle.MANIFEST).write_bytes(bundle.manifest_bytes(SHA, hashes))
        self.sign()
        with self.assertRaisesRegex(recovery.Refused, "independent manifest pin"):
            self.verify()
        self.assertFalse(self.destination.exists())

    def test_independent_signer_pin_required(self):
        for pin in ("1" * 64, "0" * 64):
            with self.assertRaises(recovery.Refused):
                self.verify(signer_pin=pin)
        self.assertFalse(self.destination.exists())

    def test_signature_wrong_namespace_refused(self):
        (self.source / bundle.SIGNATURE).unlink()
        self.ssh("-Y", "sign", "-f", str(self.key), "-n", "tor-release",
                 str(self.source / bundle.MANIFEST))
        with self.assertRaises(recovery.Refused):
            self.verify()
        self.assertFalse(self.destination.exists())

    def test_tampered_payload_and_extra_files_fail_closed(self):
        config = self.source / "install-nginx.conf"
        original = config.read_bytes()
        config.write_bytes(b"bad")
        with self.assertRaises(recovery.Refused):
            self.verify()
        config.write_bytes(original)
        (self.source / "extra.env").write_text("not allowed")
        with self.assertRaises(recovery.Refused):
            self.verify()
        self.assertFalse(self.destination.exists())

    def test_symlink_fifo_and_public_stage_refused(self):
        path = self.source / "tor-web"
        path.unlink()
        path.symlink_to(self.key)
        with self.assertRaises((ValueError, OSError)):
            self.verify()
        path.unlink()
        os.mkfifo(path)
        with self.assertRaises(recovery.Refused):
            self.verify()
        self.stage.chmod(0o755)
        with self.assertRaisesRegex(recovery.Refused, "private"):
            self.verify()
        self.assertFalse(self.destination.exists())

    def test_existing_stage_never_overwritten(self):
        self.verify()
        keep = self.destination / bundle.MANIFEST
        original = keep.read_bytes()
        with self.assertRaises(ValueError):
            self.verify()
        self.assertEqual(keep.read_bytes(), original)

    def test_explicit_v2_cli_no_legacy_fallback_or_activation_claim(self):
        cmd = [sys.executable, str(ROOT / "tools/tor_recovery_release.py"),
               "--bundle", str(self.source), "--sha", SHA,
               "--manifest-sha256", self.pin, "--allowed-signers", str(self.signers),
               "--signers-sha256", self.signer_pin]
        legacy = subprocess.run(cmd, capture_output=True, text=True, timeout=20)
        self.assertEqual(legacy.returncode, 2)
        result = subprocess.run(cmd + ["--format", "v2"], capture_output=True,
                                text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("TOR_RECOVERY_RELEASE_VERIFIED", result.stdout)
        self.assertIn("activation=false", result.stdout)
        self.assertNotIn("DR_ACCEPTED", result.stdout)


if __name__ == "__main__":
    unittest.main()
