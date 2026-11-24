"""Offline recovery release tests with real ephemeral SSH signatures."""
import hashlib
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from tor_recovery_release import BINARIES, FILES, MANIFEST, SIGNATURE, Refused, verify_release

SHA = "a" * 40


def digest(data):
    return hashlib.sha256(data).hexdigest()


class RecoveryReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.key = self.root / "fixture_key"
        subprocess.run(
            ["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(self.key)],
            check=True, capture_output=True,
        )
        public = self.key.with_suffix(".pub").read_text().split()
        self.signers = self.root / "trusted_signers"
        self.signers.write_text("tor-release " + " ".join(public[:2]) + "\n")
        self.signer_pin = digest(self.signers.read_bytes())
        for name in BINARIES:
            # These bytes are intentionally not a valid executable.
            (self.bundle / name).write_bytes(("fixture:" + name).encode())
        self.sign()
        self.manifest_pin = digest((self.bundle / MANIFEST).read_bytes())

    def sign(self):
        rows = []
        for name in BINARIES:
            row = digest((self.bundle / name).read_bytes()) + "  " + name + "\n"
            (self.bundle / (name + ".sha256")).write_text(row)
            rows.append(row)
        (self.bundle / MANIFEST).write_text("TOR_RELEASE_SHA " + SHA + "\n" + "".join(rows))
        (self.bundle / SIGNATURE).unlink(missing_ok=True)
        subprocess.run(
            ["/usr/bin/ssh-keygen", "-Y", "sign", "-f", str(self.key),
             "-n", "tor-release", str(self.bundle / MANIFEST)],
            check=True, capture_output=True,
        )

    def verify(self, **changes):
        args = dict(bundle=self.bundle, commit=SHA, manifest_pin=self.manifest_pin,
                    signers_file=self.signers, signer_pin=self.signer_pin)
        args.update(changes)
        return verify_release(**args)

    def test_offline_release_stages_exact_nonexecutable_copy(self):
        self.verify()
        self.verify(stage_root=self.root)
        stage = self.root / ("tor-recovery-" + SHA)
        self.assertEqual({p.name for p in stage.iterdir()}, FILES)
        for name in FILES:
            self.assertEqual((stage / name).read_bytes(), (self.bundle / name).read_bytes())
            self.assertEqual((stage / name).stat().st_mode & 0o777, 0o600)
        self.verify(bundle=stage)
        with self.assertRaises(FileExistsError):
            self.verify(stage_root=self.root)

    def test_wrong_commit_manifest_or_signer_pin_refused(self):
        for changes in (
            {"commit": "b" * 40},
            {"manifest_pin": "1" * 64},
            {"signer_pin": "2" * 64},
            {"manifest_pin": "0" * 64},
        ):
            with self.subTest(changes=changes), self.assertRaises(Refused):
                self.verify(**changes)

    def test_valid_resign_after_github_key_compromise_still_refused(self):
        (self.bundle / "tor-web").write_bytes(b"modified executable")
        self.sign()
        # Attacker can sign with the deployment key, but cannot alter the
        # independently retained pre-incident manifest pin.
        with self.assertRaisesRegex(Refused, "independent manifest pin"):
            self.verify()

    def test_tampered_binary_removes_partial_stage(self):
        (self.bundle / "tor-trace-store").write_bytes(b"corrupt late file")
        with self.assertRaisesRegex(Refused, "hash mismatch"):
            self.verify(stage_root=self.root)
        self.assertFalse((self.root / ("tor-recovery-" + SHA)).exists())

    def test_signature_and_checksum_tampering(self):
        sig = self.bundle / SIGNATURE
        original = sig.read_bytes()
        sig.write_bytes(b"wrong signature")
        with self.assertRaisesRegex(Refused, "signature verification"):
            self.verify()
        sig.write_bytes(original)
        (self.bundle / "tor-web.sha256").write_text("a" * 64 + "  tor-web\n")
        with self.assertRaisesRegex(Refused, "checksum file"):
            self.verify()

    def test_missing_extra_symlink_and_fifo_fail(self):
        binary = self.bundle / "tor-web"
        binary.unlink()
        with self.assertRaisesRegex(Refused, "missing or unexpected"):
            self.verify()
        binary.symlink_to(self.key)
        with self.assertRaises(OSError):
            self.verify()
        binary.unlink()
        os.mkfifo(binary)
        with self.assertRaisesRegex(Refused, "regular files"):
            self.verify()
        binary.unlink()
        binary.write_bytes(b"fixture:tor-web")
        (self.bundle / "unexpected.env").write_bytes(b"not allowed")
        with self.assertRaisesRegex(Refused, "missing or unexpected"):
            self.verify()

    def test_untrusted_stage_root_and_existing_stage_never_overwritten(self):
        target = self.root / "other"
        target.mkdir()
        target.chmod(0o777)
        with self.assertRaisesRegex(Refused, "staging root"):
            self.verify(stage_root=target)
        stage = self.root / ("tor-recovery-" + SHA)
        stage.mkdir()
        keep = stage / "keep"
        keep.write_bytes(b"existing")
        with self.assertRaises(FileExistsError):
            self.verify(stage_root=self.root)
        self.assertEqual(keep.read_bytes(), b"existing")

    def test_cli_does_not_declare_recovery_or_activation(self):
        cmd = [sys.executable, str(ROOT / "tools/tor_recovery_release.py"),
               "--bundle", str(self.bundle), "--sha", SHA,
               "--manifest-sha256", self.manifest_pin,
               "--allowed-signers", str(self.signers),
               "--signers-sha256", self.signer_pin]
        result = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("TOR_RECOVERY_RELEASE_VERIFIED", result.stdout)
        self.assertIn("activation=false", result.stdout)
        self.assertNotIn("DR_ACCEPTED", result.stdout)


if __name__ == "__main__":
    unittest.main()
