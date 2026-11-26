#!/usr/bin/env python3
"""Real disposable Nginx + HTTP fixture; no TOR instance or production data."""
from __future__ import annotations

import http.client
import http.server
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
EDGE = ROOT / "ops/fallback"
HTML = {"Accept": "text/html"}
JSON = {"Accept": "application/json"}


class Backend(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/books/504":
            time.sleep(2)
        body = b'{"code":"PROVIDER_NOT_CONFIGURED"}' if self.path.startswith("/api/") else b"catalog fixture"
        self.send_response(503 if self.path.startswith("/api/") else 200)
        self.send_header("Content-Type", "application/json" if self.path.startswith("/api/") else "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *_):
        pass


def available_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def start_backend(port):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Backend)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread


def stop_backend(server, thread):
    server.shutdown()
    server.server_close()
    thread.join(timeout=3)


def request(port, path, method="GET", headers=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=6)
    try:
        conn.request(method, path, headers=headers or HTML)
        response = conn.getresponse()
        return response.status, dict((k.lower(), v) for k, v in response.getheaders()), response.read()
    finally:
        conn.close()


def game_response(result, code):
    status, headers, body = result
    assert status == code, result
    assert "text/html" in headers["content-type"], headers
    assert headers["cache-control"] == "no-store", headers
    assert "frame-ancestors 'none'" in headers["content-security-policy"], headers
    assert headers["x-robots-tag"] == "noindex, nofollow", headers
    assert "Книжная змейка".encode() in body, body[:300]


def browser(port, site, mode):
    env = dict(os.environ, TOR_FALLBACK_TEST_ORIGIN=f"http://127.0.0.1:{port}",
               TOR_FALLBACK_TEST_CONFIG=str(site / "config.json"), TOR_FALLBACK_TEST_MODE=mode)
    subprocess.run(["node", str(ROOT / "tests/edge/fallback_browser.cjs")],
                   cwd=ROOT, env=env, check=True, timeout=90)


def main():
    with tempfile.TemporaryDirectory(prefix="tor-fallback-") as tmp:
        path = Path(tmp)
        site = path / "site"
        shutil.copytree(EDGE / "site", site)
        for name in ("nginx-http.conf", "nginx-proxy.conf", "nginx-server.conf"):
            source = (EDGE / name).read_text()
            (path / name).write_text(source.replace("/var/lib/tor-fallback", str(site)))
        port = available_port()
        upstream, thread = start_backend(0)
        backend_port = upstream.server_port
        conf = path / "nginx.conf"
        conf.write_text(f"""
worker_processes 1;
pid {path}/nginx.pid;
error_log {path}/error.log info;
events {{ worker_connections 64; }}
http {{
    access_log off;
    include /etc/nginx/mime.types;
    client_body_temp_path {path}/body;
    proxy_temp_path {path}/proxy;
    fastcgi_temp_path {path}/fastcgi;
    uwsgi_temp_path {path}/uwsgi;
    scgi_temp_path {path}/scgi;
    include {path}/nginx-http.conf;
    server {{
        listen 127.0.0.1:{port};
        server_name localhost;
        include {path}/nginx-server.conf;
        location = /books/503 {{
            include {path}/nginx-proxy.conf;
            return 503;
        }}
        location / {{
            include {path}/nginx-proxy.conf;
            proxy_pass http://127.0.0.1:{backend_port};
            proxy_connect_timeout 300ms;
            proxy_read_timeout 300ms;
        }}
    }}
}}
""")
        cmd = ["nginx", "-p", str(path), "-c", str(conf)]
        process = None
        try:
            subprocess.run(cmd + ["-t"], check=True)
            process = subprocess.Popen(cmd + ["-g", "daemon off;"])
            for _ in range(50):
                try:
                    if request(port, "/catalog")[0] == 200:
                        break
                except OSError:
                    time.sleep(.1)
            else:
                raise AssertionError("Nginx did not start")
            assert request(port, "/")[2] == b"catalog fixture"
            upstream_error = request(port, "/api/v1/books", headers=JSON)
            assert upstream_error[0] == 503
            assert upstream_error[2] == b'{"code":"PROVIDER_NOT_CONFIGURED"}'
            assert request(port, "/api/v1/books")[2] == upstream_error[2]
            game_response(request(port, "/books/503"), 503)
            game_response(request(port, "/books/504"), 504)
            stop_backend(upstream, thread)
            upstream = None
            game_response(request(port, "/"), 502)
            game_response(request(port, "/catalog?sort=new"), 502)
            game_response(request(port, "/books/123"), 502)
            status, headers, body = request(port, "/catalog", method="HEAD")
            assert status == 502 and not body and "text/html" in headers["content-type"]
            for url, method, accept in (
                ("/api/v1/books", "GET", HTML),
                ("/odmen", "GET", HTML),
                ("/readyz", "GET", HTML),
                ("/assets/example.js", "GET", HTML),
                ("/catalog", "POST", HTML),
                ("/catalog", "GET", JSON),
            ):
                status, headers, body = request(port, url, method, accept)
                assert status == 502 and "text/plain" in headers["content-type"], (url, status, headers)
                assert b"service temporarily unavailable" in body
            for asset in ("index.html", "plain.txt", "missing", "../config.json"):
                if asset.startswith(".."):
                    continue  # Not within the reserved prefix after URI normalization.
                assert request(port, "/_tor-fallback/" + asset)[0] == 404
            for asset, mime in (("style.css", "text/css"), ("game.js", "javascript"), ("config.json", "application/json")):
                status, headers, body = request(port, "/_tor-fallback/" + asset)
                assert status == 200 and mime in headers["content-type"] and body
                assert headers["cache-control"] == "no-store"
            print("HTTP: live upstream, original 502/503/504, API/write exclusions, internal paths and CSP passed", flush=True)
            browser(port, site, "offline")
            upstream, thread = start_backend(backend_port)
            browser(port, site, "recovery")
            print("FALLBACK disposable edge acceptance passed", flush=True)
        except Exception:
            if (path / "error.log").exists():
                print((path / "error.log").read_text()[-12000:])
            raise
        finally:
            if process is not None:
                process.terminate()
                process.wait(timeout=10)
            if upstream is not None:
                stop_backend(upstream, thread)


if __name__ == "__main__":
    main()
