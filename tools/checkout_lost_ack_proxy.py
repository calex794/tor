"""S034 test-only TCP proxy: forward checkout into live Axum, discard its reply.

Does not inspect or persist session tokens or private chapter content. Only a
disposable loopback HTTP server should ever be used with this test helper.
"""
from __future__ import annotations

import http.client
import socket
import threading
from collections.abc import Callable


def committed_without_ack(send: Callable[[str], object]) -> dict:
    """Call send(proxy_base); return the upstream receipt unseen by its client."""
    captured: dict = {}
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(25)
        proxy_base = f"http://127.0.0.1:{listener.getsockname()[1]}"

        def serve_once() -> None:
            try:
                downstream, _ = listener.accept()
                with downstream:
                    downstream.settimeout(20)
                    with socket.create_connection(("127.0.0.1", 3098), timeout=20) as upstream:
                        upstream.settimeout(20)
                        header = bytearray()
                        while b"\r\n\r\n" not in header:
                            part = downstream.recv(8192)
                            if not part or len(header) > 65536:
                                raise AssertionError("incomplete bounded checkout headers")
                            header.extend(part)
                        head, body = bytes(header).split(b"\r\n\r\n", 1)
                        first = head.split(b"\r\n", 1)[0]
                        if first != b"POST /api/v1/purchases HTTP/1.1":
                            raise AssertionError("wrong checkout route or HTTP method")
                        lengths = [line.split(b":", 1)[1].strip()
                                   for line in head.split(b"\r\n")[1:]
                                   if line.lower().startswith(b"content-length:")]
                        if len(lengths) != 1 or not lengths[0].isdigit():
                            raise AssertionError("expected one HTTP content length")
                        length = int(lengths[0])
                        if length < 1 or length > 4096 or len(body) > length:
                            raise AssertionError("invalid purchase request size")
                        upstream.sendall(header)
                        while len(body) < length:
                            part = downstream.recv(length - len(body))
                            if not part:
                                raise AssertionError("truncated checkout body")
                            upstream.sendall(part)
                            body += part
                        reply = http.client.HTTPResponse(upstream, method="POST")
                        reply.begin()
                        data = reply.read(65537)
                        if reply.status != 200 or len(data) > 65536:
                            raise AssertionError("live checkout failed before lost ACK")
                        captured["receipt"] = data
                        captured["cache_control"] = reply.getheader("cache-control")
                        # Reply was fully received upstream; drop every response
                        # byte on the independent buyer-facing TCP connection.
                    downstream.shutdown(socket.SHUT_WR)
            except Exception as exc:
                captured["proxy_error"] = exc

        thread = threading.Thread(target=serve_once, daemon=True)
        thread.start()
        client_error = None
        try:
            send(proxy_base)
        except (http.client.RemoteDisconnected, ConnectionResetError,
                http.client.BadStatusLine) as exc:
            client_error = exc
        thread.join(timeout=25)
        if thread.is_alive():
            raise AssertionError("checkout proxy failed to finish")
        if "proxy_error" in captured:
            raise AssertionError("live checkout forwarding failed") from captured["proxy_error"]
        if client_error is None:
            raise AssertionError("client unexpectedly received HTTP receipt")
        if "receipt" not in captured:
            raise AssertionError("missing upstream proof of successful COMMIT")
    return captured
