"""DEPLOY-01: execute the actual fixed-list builder and SSH signature verifier."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import release_bundle_v2 as bundle

SHA = "a" * 40


class ReleaseBundleTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.repo = self.root / "repo"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in bundle.BINARIES:
            (self.bin / name).write_bytes(("test executable " + name).encode())
        for name, rel in bundle.EDGE.items():
            path = self.repo / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("test asset " + name).encode())
        # A runtime-only setting must never enter the allowlisted release.
        (self.repo / "secrets.env").write_text("test-only-never-package")
        self.key = self.root / "ci_key"
        self.signers = self.root / "allowed_signers"
        self.run_ssh("-t", "ed25519", "-N", "", "-f", str(self.key))
        self.signers.write_text("tor-release " +
                                self.key.with_suffix(".pub").read_text())
        self.inbox = self.root / "inbox"
        bundle.build(self.repo, self.bin, self.inbox, SHA)
        self.sign()

    def run_ssh(self, *args):
        subprocess.run(["ssh-keygen", *args], check=True, timeout=15,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def sign(self, namespace=bundle.NAMESPACE):
        sig = self.inbox / bundle.SIGNATURE
        sig.unlink(missing_ok=True)
        self.run_ssh("-Y", "sign", "-f", str(self.key), "-n", namespace,
                     str(self.inbox / bundle.MANIFEST))

    def stage(self, sha=SHA):
        target = self.root / "verified"
        bundle.stage(self.inbox, target, sha, self.signers)
        return target

    def assert_refused(self, sha=SHA):
        with self.assertRaises((bundle.BundleError, OSError)):
            self.stage(sha)
        self.assertFalse((self.root / "verified").exists())
        self.assertFalse(list(self.root.glob(".tor-bundle-*")))

    def test_signed_bundle_covers_every_file_and_copies_independently(self):
        target = self.stage()
        self.assertEqual(set(p.name for p in target.iterdir()), bundle.FILES)
        self.assertNotIn("secrets.env", bundle.FILES)
        self.assertNotIn("config.json", bundle.FILES)
        hashes = {name: hashlib.sha256((target / name).read_bytes()).hexdigest()
                  for name in bundle.PAYLOAD}
        self.assertEqual((target / bundle.MANIFEST).read_bytes(),
                         bundle.manifest_bytes(SHA, hashes))
        for name in bundle.BINARIES:
            self.assertEqual((target / name).stat().st_mode & 0o777, 0o755)
        for name in bundle.EDGE:
            self.assertEqual((target / name).stat().st_mode & 0o777, 0o644)
        shutil.rmtree(self.inbox)
        bundle.verify(target, SHA, self.signers)

    def test_manifest_is_deterministic_for_same_checkout_and_sha(self):
        another = self.root / "another"
        bundle.build(self.repo, self.bin, another, SHA)
        self.assertEqual((another / bundle.MANIFEST).read_bytes(),
                         (self.inbox / bundle.MANIFEST).read_bytes())

    def test_altered_nginx_config_rejected(self):
        (self.inbox / "nginx-server.conf").write_text("tampered")
        self.assert_refused()

    def test_altered_binary_rejected(self):
        (self.inbox / "tor-web").write_text("tampered")
        self.assert_refused()

    def test_different_sha_rejected(self):
        self.assert_refused("b" * 40)

    def test_wrong_signer_rejected(self):
        other = self.root / "other_key"
        self.run_ssh("-t", "ed25519", "-N", "", "-f", str(other))
        self.signers.write_text("tor-release " +
                                other.with_suffix(".pub").read_text())
        self.assert_refused()

    def test_v1_namespace_cannot_authorize_v2(self):
        self.sign(namespace="tor-release")
        self.assert_refused()

    def test_missing_and_unexpected_entry_rejected(self):
        path = self.inbox / "fallback-game.js"
        contents = path.read_bytes()
        path.unlink()
        self.assert_refused()
        path.write_bytes(contents)
        (self.inbox / "unexpected.conf").write_text("not part of package")
        self.assert_refused()

    def test_even_signed_manifest_with_extra_path_is_rejected(self):
        manifest = self.inbox / bundle.MANIFEST
        with manifest.open("ab") as output:
            output.write(b"0" * 64 + b"  ../../etc/nginx/nginx.conf\n")
        self.sign()
        self.assert_refused()

    def test_symlink_and_fifo_rejected_without_reading(self):
        path = self.inbox / "fallback-game.js"
        original = path.read_bytes()
        path.unlink()
        external = self.root / "external"
        external.write_bytes(original)
        path.symlink_to(external)
        self.assert_refused()
        path.unlink()
        os.mkfifo(path)
        self.assert_refused()

    def test_oversized_metadata_rejected(self):
        (self.inbox / bundle.MANIFEST).write_bytes(b"x" * 4097)
        self.assert_refused()

    def test_existing_target_is_never_replaced(self):
        target = self.stage()
        with self.assertRaises(bundle.BundleError):
            self.stage()
        bundle.verify(target, SHA, self.signers)

    def test_untrusted_destination_parent_is_rejected(self):
        shared = self.root / "shared"
        shared.mkdir()
        shared.chmod(0o777)
        with self.assertRaises(bundle.BundleError):
            bundle.stage(self.inbox, shared / "candidate", SHA, self.signers)
        self.assertFalse((shared / "candidate").exists())

    def test_missing_build_source_leaves_no_partial_package(self):
        (self.repo / bundle.EDGE["nginx-server.conf"]).unlink()
        dest = self.root / "broken-build"
        with self.assertRaises(OSError):
            bundle.build(self.repo, self.bin, dest, SHA)
        self.assertFalse(dest.exists())
        self.assertFalse(list(self.root.glob(".tor-bundle-*")))


if __name__ == "__main__":
    unittest.main()
