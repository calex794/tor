#!/usr/bin/env python3
"""DEPLOY-02b: signed fixture releases through real isolated TLS Nginx."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))

import adopt_edge_vhost as adoption
import managed_edge_release as edge
import release_bundle_v2 as bundle
from fallback_smoke import game_response, start_backend, stop_backend

A, B, C, D = ("a" * 40, "b" * 40, "c" * 40, "d" * 40)
HOST = "pandaolate.ru"
HSTS = "max-age=31536000; includeSubDomains"


def run(*args):
    return subprocess.run(args, capture_output=True, check=True, timeout=20)


def isolated_network():
    if os.geteuid() != 0:
        raise RuntimeError("requires isolated root lab")
    if os.readlink("/proc/self/ns/net") == os.readlink("/proc/1/ns/net"):
        raise RuntimeError("refusing to run in host network namespace")
    interfaces = json.loads(run("ip", "-j", "link", "show").stdout)
    if [entry["ifname"] for entry in interfaces] != ["lo"]:
        raise RuntimeError("lab must have loopback only")


class LabNginx(adoption.ProductionAdoption):
    def __init__(self, root):
        self.root = root

    def nginx_command(self, *args):
        return ["nginx", "-p", str(self.root) + "/", "-c",
                str(self.root / "nginx.conf"), *args]

    def master_pid(self):
        return int((self.root / "nginx.pid").read_text())

    def reload_command(self):
        self.command(self.nginx_command("-s", "reload"))


def request(route, accept="text/html", method="GET"):
    result = run("curl", "--disable", "--silent", "--show-error", "--http1.1",
                 "--max-time", "5", "--max-filesize", "2097152", "--noproxy", "*",
                 "--resolve", HOST + ":443:127.0.0.1", "--include",
                 "--request", method, "--header", "Accept: " + accept,
                 "https://" + HOST + route)
    head, sep, body = result.stdout.partition(b"\r\n\r\n")
    assert sep
    first, *lines = head.split(b"\r\n")
    status = int(first.split()[1])
    headers = {}
    for line in lines:
        name, _, value = line.partition(b":")
        headers[name.decode("ascii").lower()] = value.decode("ascii").strip()
    return status, headers, body


def wait_status(path, expected):
    for _ in range(30):
        try:
            result = request(path)
            if result[0] == expected:
                return result
        except subprocess.SubprocessError:
            pass
        time.sleep(0.2)
    raise AssertionError(f"{path}: expected {expected}")


def fixture(root):
    cert, key = root / "tls.crt", root / "tls.key"
    run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
        "-subj", "/CN=" + HOST, "-addext", "subjectAltName=DNS:" + HOST,
        "-keyout", str(key), "-out", str(cert))
    os.environ["CURL_CA_BUNDLE"] = str(cert)
    vhost = root / "pandaolate.conf"
    vhost.write_text(f"""
# No production configuration: dedicated disposable TLS vhost.
server {{
    listen 127.0.0.1:443 ssl;
    server_name {HOST};
    ssl_certificate {cert};
    ssl_certificate_key {key};
    add_header Strict-Transport-Security "{HSTS}" always;
    add_header X-Frame-Options DENY always;
    location = /internal/metrics {{ return 404; }}
    location / {{
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host {HOST};
        proxy_connect_timeout 300ms;
        proxy_read_timeout 300ms;
    }}
}}
""")
    vhost.chmod(0o640)
    (root / "nginx.conf").write_text(f"""
user www-data;
worker_processes 1;
daemon off;
pid {root}/nginx.pid;
error_log {root}/error.log info;
events {{ worker_connections 64; }}
http {{
    access_log off;
    include /etc/nginx/mime.types;
    client_body_temp_path {root}/body;
    proxy_temp_path {root}/proxy;
    fastcgi_temp_path {root}/fastcgi;
    uwsgi_temp_path {root}/uwsgi;
    scgi_temp_path {root}/scgi;
    include {vhost};
}}
""")
    return vhost


def signed_releases(root):
    key = root / "lab-signing"
    run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
    signers = root / "allowed_signers"
    signers.write_text(bundle.IDENTITY + " " + key.with_suffix(".pub").read_text())
    binaries = root / "binaries"
    binaries.mkdir()
    for name in bundle.BINARIES:
        (binaries / name).write_text("inert release fixture: " + name)
    sources = root / "sources"
    sources.mkdir()
    incoming = root / "incoming"
    incoming.mkdir()
    staged = root / "staged"
    staged.mkdir()
    for sha in (A, B, C, D):
        repo = sources / sha
        fallback = repo / "ops" / "fallback"
        shutil.copytree(ROOT / "ops" / "fallback", fallback)
        if sha == B:
            with (fallback / "site" / "game.js").open("a") as out:
                out.write("\n// DEPLOY-02b second release fixture\n")
        if sha == C:
            with (fallback / "nginx-http.conf").open("a") as out:
                out.write("\ninvalid_tor_ci_directive;\n")
        if sha == D:
            server = fallback / "nginx-server.conf"
            text = server.read_text()
            location = "location = /_tor-fallback/game.js {"
            assert text.count(location) == 1
            server.write_text(text.replace(location, location + "\n    return 403;"))
        dest = incoming / sha
        bundle.build(repo, binaries, dest, sha)
        run("ssh-keygen", "-Y", "sign", "-f", str(key), "-n", bundle.NAMESPACE,
            str(dest / bundle.MANIFEST))
        bundle.stage(dest, staged / sha, sha, signers)
    return staged, signers


def expect_error(call, expected):
    try:
        call()
    except edge.EdgeError as exc:
        assert expected in str(exc), str(exc)
        print(expected, flush=True)
    else:
        raise AssertionError("failure was incorrectly accepted")


def assert_live(store, nginx, sha):
    assert store.active_sha() == sha
    nginx.probe(sha, store.releases / sha)
    status, headers, body = request("/_tor-fallback/game.js")
    assert status == 200 and headers[edge.MARKER.lower()] == sha
    assert headers["strict-transport-security"] == HSTS
    assert body == (store.releases / sha / "site/game.js").read_bytes()
    offline = request("/catalog")
    game_response(offline, 502)
    assert offline[1]["strict-transport-security"] == HSTS
    assert request("/internal/metrics")[0] == 404
    assert request("/_tor-fallback/index.html")[0] == 404
    for path, accept, method in (
        ("/api/v1/books", "application/json", "GET"),
        ("/odmen", "text/html", "GET"),
        ("/catalog", "text/html", "POST"),
    ):
        status, headers, body = request(path, accept, method)
        assert status == 502 and "text/plain" in headers["content-type"]
        assert headers["strict-transport-security"] == HSTS
        assert b"service temporarily unavailable" in body


def main():
    isolated_network()
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="tor-managed-edge-") as tmp:
        root = Path(tmp)
        root.chmod(0o755)
        vhost = fixture(root)
        original = vhost.read_bytes()
        staged, signers = signed_releases(root)
        nginx = LabNginx(root)
        store = edge.EdgeStore(root / "edge")
        backend = start_backend(3000)
        process = None
        try:
            nginx.test()
            process = subprocess.Popen(nginx.nginx_command(),
                                       stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL)
            wait_status("/catalog", 200)
            master = (root / "nginx.pid").read_text()
            assert request("/catalog")[1]["strict-transport-security"] == HSTS
            stop_backend(*backend)
            backend = None
            wait_status("/catalog", 502)
            assert "Книжная змейка".encode() not in request("/catalog")[2]
            with store.locked():
                def adopt(sha):
                    return adoption.adopt(store, staged / sha, sha, signers,
                                          vhost, adoption.hash_bytes(original), nginx)

                # Invalid syntax and a syntactically valid but unservable asset.
                for sha in (C, D):
                    expect_error(lambda: adopt(sha), "ADOPTION_ROLLBACK_VERIFIED")
                    assert vhost.read_bytes() == original
                    assert vhost.stat().st_mode & 0o777 == 0o640
                    assert not os.path.lexists(store.current)
                    assert request("/_tor-fallback/game.js")[0] == 502
                    assert request("/catalog")[0] == 502
                backup = adopt(A)
                assert backup.read_bytes() == original
                assert backup.stat().st_mode & 0o777 == 0o600
                assert_live(store, nginx, A)
                print("PASS: real first adoption with backend offline", flush=True)

                settings = store.settings / "config.json"
                settings.write_text('{"telegram_url":"https://t.me/tor_edge_fixture"}\n')
                persistent = settings.read_bytes()
                for sha in (B, C, D):
                    store.prepare(staged / sha, sha, signers)
                assert store.activate(B, nginx) == A
                assert_live(store, nginx, B)
                assert (store.releases / A / "site/game.js").read_bytes() != (
                    store.releases / B / "site/game.js").read_bytes()
                assert request("/_tor-fallback/config.json")[2] == persistent
                print("PASS: real signed A-to-B switch and persistent settings", flush=True)

                for sha in (C, D):
                    expect_error(lambda: store.activate(sha, nginx), "EDGE_ROLLBACK_VERIFIED")
                    assert_live(store, nginx, B)
                    assert settings.read_bytes() == persistent
                assert store.activate(A, nginx) == B
                assert_live(store, nginx, A)
                print("PASS: real syntax, post-reload and explicit rollback", flush=True)

            backend = start_backend(3000)
            assert wait_status("/catalog", 200)[2] == b"catalog fixture"
            status, headers, body = request("/api/v1/books", "application/json")
            assert status == 503 and body == b'{"code":"PROVIDER_NOT_CONFIGURED"}'
            assert "application/json" in headers["content-type"]
            assert (root / "nginx.pid").read_text() == master
            assert process.poll() is None
            print("PASS: backend recovery, upstream API pass-through, same Nginx master", flush=True)
            print("DEPLOY-02b REAL TLS NGINX ACCEPTANCE PASSED", flush=True)
        except Exception:
            log = root / "error.log"
            if log.exists():
                print(log.read_text()[-12000:], flush=True)
            raise
        finally:
            if process is not None and process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            if backend is not None:
                stop_backend(*backend)


if __name__ == "__main__":
    main()
