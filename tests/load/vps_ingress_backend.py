"""Disposable HTTP fixture for TOR edge-over-WireGuard CI; no production data."""

import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BODY_SIZES = {
    "/4k": 4 * 1024,
    "/64k": 64 * 1024,
    "/256k": 256 * 1024,
}
BODY = {path: b"z" * size for path, size in BODY_SIZES.items()}


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path == "/headers":
            body = json.dumps(
                {
                    "real_ip": self.headers.get("X-Real-IP", ""),
                    "forwarded_for": self.headers.get("X-Forwarded-For", ""),
                    "forwarded_proto": self.headers.get("X-Forwarded-Proto", ""),
                    "forwarded": self.headers.get("Forwarded", ""),
                    "forwarded_host": self.headers.get("X-Forwarded-Host", ""),
                    "forwarded_server": self.headers.get("X-Forwarded-Server", ""),
                }
            ).encode("utf-8")
            content_type = "application/json"
        elif self.path in BODY:
            body = BODY[self.path]
            content_type = "application/octet-stream"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            return

    def log_message(self, _format, *_args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer((os.environ.get("TOR_TEST_LISTEN_HOST", "10.88.0.2"), int(os.environ.get("TOR_TEST_LISTEN_PORT", "18081"))), FixtureHandler).serve_forever()
