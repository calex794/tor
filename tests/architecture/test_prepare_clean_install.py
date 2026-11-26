"""Signed offline preparation; never activates a live service."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import prepare_clean_install as clean
import release_bundle_v2 as bundle

SHA = "a" * 40
MACHINE = "b" * 32


class CleanInstallTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        binaries = self.root / "bin"
        binaries.mkdir()
        for name in bundle.BINARIES:
            (binaries / name).write_text("inert fixture " + name)
        self.source = self.root / "signed"
        bundle.build(ROOT, binaries, self.source, SHA)
        key = self.root / "key"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(key))
        self.signers = self.root / "allowed_signers"
        self.signers.write_text(bundle.IDENTITY + " " + key.with_suffix(".pub").read_text())
        self.ssh("-Y", "sign", "-f", str(key), "-n", bundle.NAMESPACE,
                 str(self.source / bundle.MANIFEST))
        self.pin = hashlib.sha256((self.source / bundle.MANIFEST).read_bytes()).hexdigest()

    def ssh(self, *args):
        subprocess.run(["ssh-keygen", *args], check=True, capture_output=True, timeout=15)

    def prepare(self, pin=None):
        return clean.prepare(self.root, self.source, SHA, self.signers,
                             self.pin if pin is None else pin, MACHINE)

    def test_prepare_signed_private_workspace(self):
        target = self.prepare()
        self.assertEqual(target.stat().st_mode & 0o777, 0o700)
        self.assertEqual((target / "bundle").stat().st_mode & 0o777, 0o700)
        self.assertEqual((target / "install-plan.json").stat().st_mode & 0o777, 0o600)
        record = json.loads((target / "install-plan.json").read_text())
        self.assertEqual(record["mode"], "INSTALL_EMPTY")
        self.assertEqual(record["release_sha"], SHA)
        self.assertEqual(record["manifest_sha256"], self.pin)
        for field in ("packages_installed", "database_initialized", "services_started",
                      "dns_changed", "production_accepted"):
            self.assertIs(record[field], False)
        for name in clean.DESTINATIONS:
            self.assertEqual((target / "config" / name).read_bytes(),
                             (self.source / name).read_bytes())

    def test_pin_mismatch_refused_without_workspace(self):
        with self.assertRaisesRegex(clean.Refused, "approved pin"):
            self.prepare("0" * 64)
        self.assertFalse((self.root / clean.NAME).exists())

    def test_tampered_clean_host_template_refused(self):
        (self.source / "install-nginx.conf").write_text("tampered")
        with self.assertRaises(bundle.BundleError):
            self.prepare()
        self.assertFalse((self.root / clean.NAME).exists())

    def test_existing_workspace_never_replaced(self):
        target = self.prepare()
        previous = (target / "install-plan.json").read_bytes()
        with self.assertRaisesRegex(clean.Refused, "already exists"):
            self.prepare()
        self.assertEqual((target / "install-plan.json").read_bytes(), previous)

    def test_existing_live_or_recovery_marker_refused_even_symlink(self):
        marker = self.root / "live"
        marker.symlink_to(self.root / "missing")
        with self.assertRaisesRegex(clean.Refused, "existing TOR"):
            clean.absent((marker,))

    def test_target_identity_must_match(self):
        for value in ("0" * 32, "c" * 32, "../"):
            with self.assertRaises(clean.Refused):
                clean.validate_host_id(value, MACHINE)
        clean.validate_host_id(MACHINE, MACHINE)

    def test_read_only_verification_does_not_prepare(self):
        clean.verified_source(self.source, SHA, self.signers, self.pin)
        self.assertFalse((self.root / clean.NAME).exists())

    def test_clean_base_templates_preserve_security_boundaries(self):
        nginx = (ROOT / bundle.INSTALL["install-nginx.conf"]).read_text()
        unit = (ROOT / bundle.INSTALL["install-tor-web.service"]).read_text()
        pg = (ROOT / bundle.INSTALL["install-postgres.conf"]).read_text()
        for name in ("nginx-http.conf", "nginx-server.conf", "nginx-proxy.conf"):
            self.assertEqual(nginx.count("/var/lib/tor-edge/current/" + name), 1)
        for path in ("/healthz", "/readyz"):
            block = nginx.split("location = " + path + " {", 1)[1].split("}", 1)[0]
            self.assertIn("allow 127.0.0.1;", block)
            self.assertIn("deny all;", block)
        self.assertIn("location = /metrics { return 404; }", nginx)
        self.assertIn("proxy_set_header X-Real-IP $remote_addr;", nginx)
        self.assertIn("proxy_set_header Forwarded \"\";", nginx)
        self.assertIn("proxy_buffering off;", nginx)
        self.assertIn("ssl_reject_handshake on;", nginx)
        self.assertIn("User=tor-deploy", unit)
        self.assertIn("ExecStart=/opt/tor/current/tor-web", unit)
        self.assertIn("Environment=TOR_LISTEN_ADDR=127.0.0.1:3000", unit)
        self.assertIn("Environment=TOR_DEMO_MODE=0", pg)
        self.assertIn("Environment=TOR_AUTH_ENABLED=1", pg)
        self.assertNotIn("DATABASE_URL=", pg)


if __name__ == "__main__":
    unittest.main()
