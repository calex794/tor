"""Offline export must never connect to a VPS; import uses independent pins."""
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import import_offline_release as offline
import release_bundle_v2 as bundle

SHA = "a" * 40
WORKFLOW = (ROOT / ".github/workflows/tor-one-click-release.yml").read_text()


class ImportTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        binaries = self.root / "bin"
        binaries.mkdir()
        for name in bundle.BINARIES:
            (binaries / name).write_bytes(b"inert " + name.encode())
        self.source = self.root / "extracted"
        bundle.build(ROOT, binaries, self.source, SHA)
        key = self.root / "key"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(key))
        self.signers = self.root / "allowed_signers"
        self.signers.write_text(bundle.IDENTITY + " " + key.with_suffix(".pub").read_text())
        self.ssh("-Y", "sign", "-f", str(key), "-n", bundle.NAMESPACE,
                 str(self.source / bundle.MANIFEST))
        self.pin = hashlib.sha256((self.source / bundle.MANIFEST).read_bytes()).hexdigest()
        self.destination = self.root / "verified"

    def ssh(self, *args):
        subprocess.run(["ssh-keygen", *args], check=True, capture_output=True, timeout=15)

    def load(self, pin=None, signers=None):
        offline.import_release(self.source, self.destination, SHA,
                               signers or self.signers, pin or self.pin)

    def test_roundtrip_restores_executable_modes_and_exact_payload(self):
        # GitHub artifact ZIP downloads don't preserve executable permissions.
        for path in self.source.iterdir():
            path.chmod(0o644)
        self.load()
        bundle.verify(self.destination, SHA, self.signers)
        self.assertEqual(self.destination.stat().st_mode & 0o777, 0o700)
        self.assertEqual({p.name for p in self.destination.iterdir()}, bundle.FILES)
        for path in self.destination.iterdir():
            self.assertEqual(path.stat().st_mode & 0o777, 0o755 if path.name in bundle.BINARIES else 0o644)

    def test_wrong_pin_never_publishes(self):
        with self.assertRaises(offline.ImportError):
            self.load("0" * 64)
        self.assertFalse(self.destination.exists())

    def test_wrong_signer_never_publishes(self):
        key = self.root / "other"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(key))
        signers = self.root / "other_signers"
        signers.write_text(bundle.IDENTITY + " " + key.with_suffix(".pub").read_text())
        with self.assertRaises(bundle.BundleError):
            self.load(signers=signers)
        self.assertFalse(self.destination.exists())

    def test_tampered_extra_or_linked_payload_refused(self):
        path = self.source / "install-nginx.conf"
        original = path.read_bytes()
        path.write_bytes(b"tampered")
        with self.assertRaises(bundle.BundleError):
            self.load()
        path.write_bytes(original)
        extra = self.source / "private-key"
        extra.write_bytes(b"fixture")
        with self.assertRaises(bundle.BundleError):
            self.load()
        extra.unlink()
        path.unlink()
        path.symlink_to(self.signers)
        with self.assertRaises((ValueError, OSError)):
            self.load()
        self.assertFalse(self.destination.exists())

    def test_existing_destination_never_overwritten(self):
        self.load()
        original = (self.destination / bundle.MANIFEST).read_bytes()
        with self.assertRaises(bundle.BundleError):
            self.load()
        self.assertEqual((self.destination / bundle.MANIFEST).read_bytes(), original)


class WorkflowTests(unittest.TestCase):
    def test_all_remote_steps_explicitly_exclude_export(self):
        for block in WORKFLOW.split("      - name: ")[1:]:
            if any(fragment in block for fragment in ('ssh "${opts[@]}"', 'scp -p "${opts[@]}"')):
                self.assertIn("inputs.export_v2 == false", block, block.splitlines()[0])

    def test_export_still_requires_exact_main_and_acceptance(self):
        self.assertIn("false:false:false|true:false:false|false:true:false|false:false:true", WORKFLOW)
        self.assertIn("TOR_RELEASE_PROTOCOL=v2", WORKFLOW)
        self.assertIn("github.ref == 'refs/heads/main'", WORKFLOW)
        self.assertIn("environment: tor-staging", WORKFLOW)
        for before, after in (("check_one_click_release_ci.py", "Собрать сайт"),
                              ("python3 tests/edge/managed_edge_smoke.py", "Собрать и подписать весь комплект v2"),
                              ("Запретить установку устаревшего коммита", "Проверить автономный импорт"),
                              ("Проверить автономный импорт", "Сохранить полный автономный выпуск v2")):
            self.assertLess(WORKFLOW.index(before), WORKFLOW.index(after))

    def test_export_upload_only_verified_flat_payload(self):
        block = WORKFLOW.split('name: "Сохранить полный автономный выпуск v2"', 1)[1].split("      - name:", 1)[0]
        self.assertIn("if: inputs.export_v2 == true", block)
        self.assertIn("path: ${{ runner.temp }}/tor-offline-v2/*", block)
        self.assertIn("if-no-files-found: error", block)
        self.assertNotIn("tor_id_ed25519", block)
        self.assertNotIn("tor_known_hosts", block)

    def test_keys_not_present_during_build_or_edge_lab(self):
        job_env = WORKFLOW.split("    steps:", 1)[0]
        self.assertNotIn("secrets.TOR_DEPLOY_", job_env)
        for block in WORKFLOW.split("      - name: ")[1:]:
            if "secrets.TOR_DEPLOY_SSH_KEY" in block:
                self.assertIn("trap 'rm -f", block)
            if "cargo build" in block or "unshare --net" in block:
                self.assertNotIn("TOR_DEPLOY_SSH_KEY", block)


if __name__ == "__main__":
    unittest.main()
