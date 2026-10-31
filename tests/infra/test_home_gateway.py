"""Real Nginx end-to-end smoke test without an external server or real keys.

The loopback link substitutes for wg0 in CI; WireGuard itself isn't started.
"""
from __future__ import annotations

import http.server
import ipaddress
import json
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RENDER = ROOT / "tools" / "render_home_gateway.py"
DOMAIN = "books.example.test"


class Headers(http.server.BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        fields = {name.lower(): value for name, value in self.headers.items()}
        data = json.dumps(fields).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args: object) -> None:
        return


def free_port(host: str) -> int:
    with socket.socket() as conn:
        conn.bind((host, 0))
        return conn.getsockname()[1]


class HomeGatewayTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        if not shutil.which("nginx") or not shutil.which("openssl"):
            raise RuntimeError("NGINX and OpenSSL are required; CI installs them")
        cls.temp = tempfile.TemporaryDirectory(prefix="tor-home-gateway-")
        cls.work = Path(cls.temp.name)
        cls.procs = []
        cls.backend = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Headers)
        cls.thread = threading.Thread(target=cls.backend.serve_forever, daemon=True)
        cls.thread.start()
        cls.edge_port = free_port("127.0.0.1")
        cls.home_port = free_port("127.0.0.2")
        cls.cert = cls.work / "cert.pem"
        cls.key = cls.work / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-days", "1", "-keyout", str(cls.key), "-out", str(cls.cert),
            "-subj", f"/CN={DOMAIN}", "-addext", f"subjectAltName=DNS:{DOMAIN}"
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        cls.home_conf = cls.work / "home.conf"
        cls.edge_conf = cls.work / "edge.conf"
        home_args = [
            "--listen", f"127.0.0.2:{cls.home_port}",
            "--edge-ip", "127.0.0.1", "--domain", DOMAIN,
            "--backend", f"127.0.0.1:{cls.backend.server_port}",
            "--cert", str(cls.cert), "--key", str(cls.key),
            "--pid", str(cls.work / "home.pid"), "--output", str(cls.home_conf)
        ]
        edge_args = [
            "--listen", f"127.0.0.1:{cls.edge_port}",
            "--home", f"127.0.0.2:{cls.home_port}",
            "--pid", str(cls.work / "edge.pid"), "--output", str(cls.edge_conf)
        ]
        for role, args in [("home", home_args), ("edge", edge_args)]:
            subprocess.run(
                [sys.executable, str(RENDER), role, *args], check=True
            )
            conf = cls.home_conf if role == "home" else cls.edge_conf
            subprocess.run(["nginx", "-t", "-c", str(conf)],
                           check=True, capture_output=True, text=True)
            log = open(cls.work / f"{role}.log", "w", encoding="utf-8")
            proc = subprocess.Popen(
                ["nginx", "-c", str(conf), "-g", "daemon off;"],
                stdout=log, stderr=subprocess.STDOUT
            )
            cls.procs.append((proc, log))
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if any(proc.poll() is not None for proc, _ in cls.procs):
                logs = [(cls.work / f"{role}.log").read_text()
                        for role in ("home", "edge")]
                raise RuntimeError("NGINX terminated: " + repr(logs))
            try:
                with socket.create_connection(("127.0.0.1", cls.edge_port), .2):
                    break
            except OSError:
                time.sleep(.05)
        else:
            raise RuntimeError("gateway did not listen")

    @classmethod
    def tearDownClass(cls) -> None:
        for proc, log in getattr(cls, "procs", []):
            proc.terminate()
            try:
                proc.wait(timeout=4)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)
            log.close()
        if hasattr(cls, "backend"):
            cls.backend.shutdown()
            cls.backend.server_close()
            cls.thread.join(timeout=2)
        if hasattr(cls, "temp"):
            cls.temp.cleanup()

    @classmethod
    def request(cls, host: str = DOMAIN, injected: bool = True) -> tuple[int, bytes]:
        context = ssl.create_default_context(cafile=str(cls.cert))
        with socket.create_connection(("127.0.0.1", cls.edge_port), timeout=3) as raw:
            with context.wrap_socket(raw, server_hostname=DOMAIN) as tls:
                headers = [
                    "GET /test-path HTTP/1.1", f"Host: {host}",
                    "Origin: https://books.example.test", "Connection: close",
                ]
                if injected:
                    headers.extend([
                        "X-Real-IP: 198.51.100.77",
                        "X-Forwarded-For: 198.51.100.88",
                        "Forwarded: for=198.51.100.99",
                    ])
                tls.sendall(("\r\n".join(headers) + "\r\n\r\n").encode())
                data = bytearray()
                while True:
                    part = tls.recv(8192)
                    if not part:
                        break
                    data.extend(part)
        head, sep, body = bytes(data).partition(b"\r\n\r\n")
        if not sep:
            raise AssertionError("HTTP response missing headers")
        code = int(head.split(b" ", 2)[1])
        return code, body

    def test_real_client_ip_and_untrusted_headers(self) -> None:
        code, body = self.request()
        self.assertEqual(code, 200)
        fields = json.loads(body)
        self.assertEqual(fields["host"], DOMAIN)
        self.assertEqual(fields["x-real-ip"], "127.0.0.1")
        self.assertEqual(fields["x-forwarded-for"], "127.0.0.1")
        self.assertEqual(fields["x-forwarded-proto"], "https")
        self.assertEqual(fields["origin"], "https://books.example.test")
        self.assertNotIn("forwarded", fields)
        self.assertNotIn("198.51.100", str(fields))

    def test_invalid_http_host_rejected_before_rust(self) -> None:
        code, _ = self.request(host="attacker.example.test")
        self.assertEqual(code, 421)

    def test_home_tls_requires_proxy_preamble(self) -> None:
        context = ssl._create_unverified_context()
        with socket.create_connection(("127.0.0.2", self.home_port), timeout=2) as raw:
            with self.assertRaises(OSError):
                with context.wrap_socket(raw, server_hostname=DOMAIN):
                    pass

    def test_render_rejects_nginx_injection_and_public_backend(self) -> None:
        output = self.work / "invalid.conf"
        basic = ["--listen", "127.0.0.1:18444", "--pid",
                 str(self.work / "invalid.pid"), "--output", str(output)]
        for args in [
            ["edge", "--home", "10.77.77.2:8443;deny all"],
            ["home", "--edge-ip", "10.77.77.1", "--backend", "0.0.0.0:3000",
             "--domain", DOMAIN, "--cert", str(self.cert), "--key", str(self.key)],
        ]:
            result = subprocess.run(
                [sys.executable, str(RENDER), *args, *basic],
                capture_output=True, text=True
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(output.exists())

    def test_wireguard_samples_restrict_routes_and_have_no_keys(self) -> None:
        examples = [
            ROOT / "ops/home-gateway/edge.wg0.conf.example",
            ROOT / "ops/home-gateway/home.wg0.conf.example",
        ]
        for path in examples:
            contents = path.read_text()
            self.assertNotIn("0.0.0.0/0", contents)
            self.assertIn("AllowedIPs = 10.77.77.", contents)
            self.assertIn("REPLACE_WITH_", contents)
        self.assertIn("PersistentKeepalive = 25", examples[1].read_text())


if __name__ == "__main__":
    unittest.main()
