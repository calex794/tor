"""DEPLOY-02/03: real signed package and fault injection at the Nginx boundary."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import managed_edge_release as edge
import release_bundle_v2 as bundle

A, B = "a" * 40, "b" * 40


class NginxFixture:
    def __init__(self, store, failure=None, rollback_failure=False):
        self.store, self.failure = store, failure
        self.rollback_failure = rollback_failure
        self.running = A
        self.calls = []

    def test(self):
        sha = self.store.active_sha()
        self.calls.append(("test", sha))
        if sha == B and self.failure == "test":
            raise RuntimeError("candidate syntax failure")

    def reload(self):
        sha = self.store.active_sha()
        self.calls.append(("reload", sha))
        if sha == B and self.failure == "reload":
            raise RuntimeError("candidate reload failure")
        if sha == A and self.rollback_failure:
            raise RuntimeError("rollback reload failure")
        self.running = sha

    def probe(self, sha, directory):
        self.calls.append(("probe", sha))
        if sha != self.running or (sha == B and self.failure == "probe"):
            raise RuntimeError("unexpected live release")
        if not (directory / "site/game.js").is_file():
            raise RuntimeError("asset missing")


class ManagedEdgeTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.repo = self.root / "repo"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.store = edge.EdgeStore(self.root / "edge")
        self.key = self.root / "key"
        self.signers = self.root / "allowed_signers"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(self.key))
        self.signers.write_text("tor-release " + self.key.with_suffix(".pub").read_text())
        for name in bundle.BINARIES:
            (self.bin / name).write_bytes(b"binary fixture")
        for name, rel in bundle.EDGE.items():
            path = self.repo / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("fixture " + name).encode())
        (self.repo / bundle.EDGE["fallback-config.example.json"]).write_text('{"telegram_url":null}')
        template = self.repo / bundle.EDGE["nginx-server.conf"]
        template.write_text("\n".join(
            f"location = /_tor-fallback/{name} {{\n    alias /var/lib/tor-fallback/{name};\n}}"
            for name in (*edge.ASSETS, "config.json")) + "\n")
        self.stage(A)
        with self.store.locked():
            self.store.prepare(self.root / A, A, self.signers)
            # Model already approved first adoption, NOT automatic vhost editing.
            self.store.switch_link(A)
        self.stage(B)

    def ssh(self, *args):
        subprocess.run(["ssh-keygen", *args], check=True, timeout=15,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def stage(self, sha):
        source = self.root / sha
        bundle.build(self.repo, self.bin, source, sha)
        self.ssh("-Y", "sign", "-f", str(self.key), "-n", bundle.NAMESPACE,
                 str(source / bundle.MANIFEST))

    def prepare(self):
        return self.store.prepare(self.root / B, B, self.signers)

    def test_prepare_is_idempotent_and_does_not_change_live_edge(self):
        with self.store.locked():
            target = self.prepare()
            self.assertEqual(target, self.prepare())
            self.assertEqual(self.store.active_sha(), A)
            self.assertIn(str(target / "site/game.js"), (target / "nginx-server.conf").read_text())
            self.assertIn(str(self.store.settings / "config.json"), (target / "nginx-server.conf").read_text())
            self.assertIn(B, (target / "nginx-server.conf").read_text())
            self.assertNotIn("/var/lib/tor-fallback/", (target / "nginx-server.conf").read_text())

    def test_operator_telegram_survives_upgrade_and_rollback(self):
        config = self.store.settings / "config.json"
        config.write_text('{"telegram_url":"https://t.me/fixture_channel"}')
        with self.store.locked():
            self.prepare()
            nginx = NginxFixture(self.store)
            self.assertEqual(self.store.activate(B, nginx), A)
            self.assertEqual(self.store.active_sha(), B)
            self.assertEqual(self.store.activate(A, nginx), B)
        self.assertIn("fixture_channel", config.read_text())

    def test_strict_root_umask_keeps_site_readable(self):
        other = edge.EdgeStore(self.root / "strict-umask")
        previous = os.umask(0o077)
        try:
            with other.locked():
                target = other.prepare(self.root / B, B, self.signers)
        finally:
            os.umask(previous)
        for path in (other.root, other.releases, other.settings, target, target / "site"):
            self.assertEqual(path.stat().st_mode & 0o777, 0o755)
        self.assertEqual((target / "site/game.js").stat().st_mode & 0o777, 0o644)

    def test_prepared_checksum_mismatch_refused_before_reload(self):
        with self.store.locked():
            target = self.prepare()
            (target / "nginx-http.conf").write_text("damaged")
            nginx = NginxFixture(self.store)
            with self.assertRaisesRegex(edge.EdgeError, "checksum mismatch"):
                self.store.activate(B, nginx)
            self.assertEqual(nginx.calls, [])
            self.assertEqual(self.store.active_sha(), A)

    def test_duplicate_public_setting_cannot_smuggle_data(self):
        self.assertFalse(edge.settings_valid(b'{"telegram_url":"secret","telegram_url":null}'))

    def test_unchanged_release_does_not_reload(self):
        with self.store.locked():
            nginx = NginxFixture(self.store)
            self.store.activate(A, nginx)
            self.assertEqual(nginx.calls, [("test", A), ("probe", A)])

    def test_signed_file_tamper_refused(self):
        (self.root / B / "nginx-server.conf").write_text("tampered")
        with self.store.locked(), self.assertRaises(bundle.BundleError):
            self.prepare()
        self.assertFalse((self.store.releases / B).exists())

    def test_managed_release_tamper_refused_on_prepare_retry(self):
        with self.store.locked():
            target = self.prepare()
            (target / "site/game.js").write_text("changed")
            with self.assertRaises(edge.EdgeError):
                self.prepare()

    def test_candidate_syntax_reload_and_probe_failures_restore_previous(self):
        with self.store.locked():
            self.prepare()
            for failure in ("test", "reload", "probe"):
                with self.subTest(failure=failure):
                    nginx = NginxFixture(self.store, failure)
                    with self.assertRaisesRegex(edge.EdgeError, "EDGE_ROLLBACK_VERIFIED"):
                        self.store.activate(B, nginx)
                    self.assertEqual(self.store.active_sha(), A)
                    self.assertEqual(nginx.running, A)
                    self.assertEqual(nginx.calls[-1], ("probe", A))

    def test_rollback_failure_is_not_reported_as_success(self):
        with self.store.locked():
            self.prepare()
            nginx = NginxFixture(self.store, "probe", rollback_failure=True)
            with self.assertRaisesRegex(edge.EdgeError, "EDGE_ROLLBACK_UNVERIFIED"):
                self.store.activate(B, nginx)
            self.assertEqual(self.store.active_sha(), A)

    def test_missing_adoption_refused_without_reload(self):
        with self.store.locked():
            self.prepare()
            self.store.current.unlink()
            nginx = NginxFixture(self.store)
            with self.assertRaisesRegex(edge.EdgeError, "adoption required"):
                self.store.activate(B, nginx)
            self.assertEqual(nginx.calls, [])

    def test_unsafe_current_symlink_refused(self):
        with self.store.locked():
            self.prepare()
            self.store.current.unlink()
            self.store.current.symlink_to("/etc/nginx")
            nginx = NginxFixture(self.store)
            with self.assertRaisesRegex(edge.EdgeError, "outside managed releases"):
                self.store.activate(B, nginx)
            self.assertEqual(nginx.calls, [])

    def test_operator_settings_symlink_and_invalid_json_refused(self):
        config = self.store.settings / "config.json"
        with self.store.locked():
            config.write_text('{"telegram_url":"https://example.com/evil"}')
            with self.assertRaises(edge.EdgeError):
                self.prepare()
            config.unlink()
            config.symlink_to(self.signers)
            with self.assertRaises(edge.EdgeError):
                self.prepare()

    def test_world_writable_candidate_is_refused(self):
        (self.root / B / "nginx-server.conf").chmod(0o666)
        with self.store.locked(), self.assertRaises(edge.EdgeError):
            self.prepare()

    def test_signed_unsupported_alias_refused(self):
        (self.repo / bundle.EDGE["nginx-server.conf"]).write_text("alias /etc/passwd;")
        sha = "c" * 40
        self.stage(sha)
        with self.store.locked(), self.assertRaises(edge.EdgeError):
            self.store.prepare(self.root / sha, sha, self.signers)
        self.assertFalse((self.store.releases / sha).exists())


if __name__ == "__main__":
    unittest.main()
