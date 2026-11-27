"""DEPLOY-02a: exact Nginx insertion, real signatures, adoption failure handling."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import adopt_edge_vhost as adoption
import managed_edge_release as edge
import release_bundle_v2 as bundle

SHA = "c" * 40
HSTS = "max-age=31536000; includeSubDomains"
VHOST = b"""# existing TLS and redirect rules; braces in comments are opaque: {}
server {
    listen 80;
    server_name pandaolate.ru;
    return 301 https://pandaolate.ru$request_uri;
}
server {
    listen 443 ssl;
    server_name pandaolate.ru;
    ssl_certificate /etc/letsencrypt/live/pandaolate.ru/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/pandaolate.ru/privkey.pem;
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    location = /internal/metrics { return 404; }
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host pandaolate.ru;
        proxy_read_timeout 60s;
    }
}
"""


class NginxFixture:
    def __init__(self, vhost, store, failure=None):
        self.vhost, self.store, self.failure = vhost, store, failure
        self.active = False
        self.calls = []
        self.reloads = 0

    def candidate(self):
        return b"nginx-server.conf;" in self.vhost.read_bytes()

    def test(self):
        self.calls.append("test")
        if self.candidate() and self.failure == "test":
            raise RuntimeError("syntax")

    def loaded(self, path):
        self.calls.append("loaded")
        if path != self.vhost or self.failure == "not-loaded":
            raise RuntimeError("not loaded")

    def reload(self):
        self.calls.append("reload")
        self.reloads += 1
        if self.candidate() and self.failure == "reload":
            raise RuntimeError("reload")
        if not self.candidate() and self.failure == "rollback":
            raise RuntimeError("rollback")
        self.active = self.candidate()

    def probe(self, sha, target):
        self.calls.append("probe")
        if not self.active or self.failure in ("probe", "rollback"):
            raise RuntimeError("probe")
        if sha != SHA or not (target / "site/game.js").is_file():
            raise RuntimeError("asset")

    def snapshot(self, route):
        headers = {"strict-transport-security": (HSTS,)}
        if route == "/catalog":
            if self.active and self.failure == "catalog":
                return adoption.Snapshot(500, headers)
            return adoption.Snapshot(502, headers)
        if self.active:
            headers[edge.MARKER.lower()] = (SHA,)
            if self.failure == "hsts":
                headers.pop("strict-transport-security")
            return adoption.Snapshot(200, headers)
        return adoption.Snapshot(502, headers)


class AdoptionTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.store = edge.EdgeStore(self.root / "edge")
        self.vhost = self.root / "pandaolate.conf"
        self.vhost.write_bytes(VHOST)
        self.vhost.chmod(0o640)
        self.repo = self.root / "repo"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in bundle.BINARIES:
            (self.bin / name).write_bytes(b"fixture binary")
        for name, rel in bundle.CONFIGS.items():
            path = self.repo / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("fixture " + name).encode())
        (self.repo / bundle.EDGE["fallback-config.example.json"]).write_text('{"telegram_url":null}')
        (self.repo / bundle.EDGE["nginx-server.conf"]).write_text("\n".join(
            f"location = /_tor-fallback/{name} {{\n    alias /var/lib/tor-fallback/{name};\n}}"
            for name in (*edge.ASSETS, "config.json")) + "\n")
        key = self.root / "key"
        self.signers = self.root / "allowed_signers"
        self.ssh("-t", "ed25519", "-N", "", "-f", str(key))
        self.signers.write_text("tor-release " + key.with_suffix(".pub").read_text())
        self.source = self.root / "bundle"
        bundle.build(self.repo, self.bin, self.source, SHA)
        self.ssh("-Y", "sign", "-f", str(key), "-n", bundle.NAMESPACE,
                 str(self.source / bundle.MANIFEST))

    def ssh(self, *args):
        subprocess.run(["ssh-keygen", *args], check=True, timeout=15,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def adopt(self, nginx, approved=None):
        with self.store.locked():
            return adoption.adopt(self.store, self.source, SHA, self.signers,
                                  self.vhost, approved or adoption.hash_bytes(VHOST), nginx)

    def test_inserts_only_three_includes(self):
        candidate = adoption.render(VHOST, self.store.root)
        expected = VHOST.replace(b"server {\n    listen 443 ssl;",
            f"include {self.store.root}/current/nginx-http.conf;\n\nserver {{\n    include {self.store.root}/current/nginx-server.conf;\n    listen 443 ssl;".encode())
        expected = expected.replace(b"location / {",
            f"location / {{\n        include {self.store.root}/current/nginx-proxy.conf;".encode())
        self.assertEqual(candidate, expected)

    def test_success_while_backend_is_down_preserves_tls_and_hsts(self):
        nginx = NginxFixture(self.vhost, self.store)
        backup = self.adopt(nginx)
        self.assertEqual(backup.read_bytes(), VHOST)
        self.assertEqual(backup.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.vhost.stat().st_mode & 0o777, 0o640)
        self.assertEqual(self.store.active_sha(), SHA)
        self.assertTrue(nginx.active)
        self.assertEqual((self.store.settings / "hsts.txt").read_text(), HSTS)
        snippet = self.store.releases / SHA / "nginx-server.conf"
        self.assertEqual(snippet.read_text().count(f'add_header Strict-Transport-Security "{HSTS}" always;'), 5)
        self.assertIn(b"ssl_certificate_key", self.vhost.read_bytes())
        self.assertIn(b"location = /internal/metrics { return 404; }", self.vhost.read_bytes())

    def test_failures_restore_exact_original_and_remove_managed_link(self):
        for failure in ("test", "reload", "probe", "catalog", "hsts"):
            with self.subTest(failure=failure):
                nginx = NginxFixture(self.vhost, self.store, failure)
                with self.assertRaisesRegex(edge.EdgeError, "ADOPTION_ROLLBACK_VERIFIED"):
                    self.adopt(nginx)
                self.assertEqual(self.vhost.read_bytes(), VHOST)
                self.assertEqual(self.vhost.stat().st_mode & 0o777, 0o640)
                self.assertFalse(os.path.lexists(self.store.current))
                self.assertFalse(nginx.active)

    def test_failed_rollback_not_reported_as_verified(self):
        nginx = NginxFixture(self.vhost, self.store, "rollback")
        with self.assertRaisesRegex(edge.EdgeError, "ADOPTION_ROLLBACK_UNVERIFIED"):
            self.adopt(nginx)
        self.assertEqual(self.vhost.read_bytes(), VHOST)
        self.assertTrue(self.store.current.is_symlink())
        self.assertTrue(nginx.active)

    def test_wrong_approval_hash_refused_before_commands(self):
        nginx = NginxFixture(self.vhost, self.store)
        with self.assertRaisesRegex(edge.EdgeError, "approved SHA-256"):
            self.adopt(nginx, "0" * 64)
        self.assertEqual(nginx.calls, [])
        self.assertEqual(self.vhost.read_bytes(), VHOST)

    def test_wrong_upstream_shared_host_and_duplicate_tls_refused(self):
        for value in (VHOST.replace(b"127.0.0.1:3000", b"10.88.0.2:18081"),
                      VHOST.replace(b"server_name pandaolate.ru;", b"server_name pandaolate.ru other.example;"),
                      VHOST + VHOST,
                      VHOST.replace(b"proxy_read_timeout 60s;", b"error_page 502 /existing.html;"),
                      VHOST.replace(b"proxy_read_timeout 60s;", b"proxy_intercept_errors off;")):
            with self.subTest():
                with self.assertRaises(edge.EdgeError):
                    adoption.render(value, self.store.root)

    def test_symlink_or_world_writable_vhost_refused(self):
        self.vhost.chmod(0o666)
        nginx = NginxFixture(self.vhost, self.store)
        with self.assertRaises(edge.EdgeError):
            self.adopt(nginx)
        self.vhost.unlink()
        target = self.root / "actual.conf"
        target.write_bytes(VHOST)
        self.vhost.symlink_to(target)
        with self.assertRaises(edge.EdgeError):
            self.adopt(nginx)
        self.assertEqual(nginx.calls, [])

    def test_existing_adoption_refused(self):
        self.adopt(NginxFixture(self.vhost, self.store))
        nginx = NginxFixture(self.vhost, self.store)
        with self.assertRaisesRegex(edge.EdgeError, "already adopted"):
            self.adopt(nginx, adoption.hash_bytes(self.vhost.read_bytes()))
        self.assertEqual(nginx.calls, [])

    def test_duplicate_or_injected_hsts_refused(self):
        for value in ('max-age=10"; include /etc/passwd;', "max-age=10; preload; preload"):
            with self.assertRaises(edge.EdgeError):
                edge.validate_hsts(value)
        with self.assertRaises(edge.EdgeError):
            adoption.Snapshot(200, {"strict-transport-security": (HSTS, HSTS)}).hsts()

    def test_healthy_catalog_security_change_refused(self):
        old = adoption.Snapshot(200, {"x-frame-options": ("DENY",)})
        with self.assertRaisesRegex(edge.EdgeError, "security headers changed"):
            adoption.compare_catalog(old, adoption.Snapshot(200, {}))

    def test_quoted_braces_and_comments_do_not_change_block_selection(self):
        old = VHOST.replace(b"proxy_read_timeout 60s;", b'proxy_set_header X-Example "{ ; } # text"; # }\n        proxy_read_timeout 60s;')
        self.assertIn(b'proxy_set_header X-Example "{ ; } # text"; # }', adoption.render(old, self.store.root))


if __name__ == "__main__":
    unittest.main()
