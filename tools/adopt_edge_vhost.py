#!/usr/bin/env python3
"""DEPLOY-02a: root-only, guarded first adoption of one existing TOR vhost."""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import time

import managed_edge_release as edge
import release_bundle_v2 as bundle

HOST = "pandaolate.ru"
MAX_CONFIG = 256 * 1024
# Quoted strings and escapes are opaque. Never search raw braces with regex.
TOKEN = re.compile(r"""#[^\r\n]*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[{};]|(?:\\.|[$]\{[A-Za-z0-9_]+\}|[^\s{};#'"\\])+""")
SECURITY = ("strict-transport-security", "content-security-policy",
            "x-frame-options", "x-content-type-options", "referrer-policy",
            "permissions-policy", "x-robots-tag")


@dataclass
class Entry:
    words: tuple[str, ...]
    start: int
    opening: int | None
    children: list[Entry]


def parse(text: str) -> list[Entry]:
    tokens = []
    last = 0
    for match in TOKEN.finditer(text):
        if text[last:match.start()].strip():
            raise edge.EdgeError("unsupported Nginx syntax")
        last = match.end()
        if not match.group().startswith("#"):
            tokens.append((match.group(), match.start(), match.end()))
    if text[last:].strip():
        raise edge.EdgeError("unsupported Nginx syntax")

    def block(index, depth):
        if depth > 64:
            raise edge.EdgeError("configuration nesting too deep")
        entries, words, start = [], [], None
        while index < len(tokens):
            token, pos, end = tokens[index]
            index += 1
            if token == "}":
                if words or depth == 0:
                    raise edge.EdgeError("invalid Nginx block")
                return entries, index
            if token in (";", "{"):
                if not words:
                    raise edge.EdgeError("empty Nginx directive")
                children, opening = [], None
                if token == "{":
                    opening = end
                    children, index = block(index, depth + 1)
                entries.append(Entry(tuple(words), start, opening, children))
                words, start = [], None
            else:
                if start is None:
                    start = pos
                words.append(token)
        if depth or words:
            raise edge.EdgeError("unterminated Nginx directive")
        return entries, index

    return block(0, 0)[0]


def value(token):
    return token[1:-1] if len(token) > 1 and token[0] == token[-1] in ("'", '"') else token


def directives(entries, name):
    return [tuple(value(x) for x in e.words[1:])
            for e in entries if e.opening is None and e.words[0] == name]


def render(original: bytes, root: Path) -> bytes:
    if b"\x00" in original or b"\r" in original:
        raise edge.EdgeError("unsupported vhost encoding or line endings")
    text = original.decode("utf-8")
    if "tor-edge/current/" in text or "/_tor-fallback/" in text:
        raise edge.EdgeError("existing managed or conflicting fallback configuration")
    if not edge.PATH.fullmatch(str(root)):
        raise edge.EdgeError("unsafe managed include path")
    servers = []
    for entry in parse(text):
        if entry.words != ("server",) or entry.opening is None:
            continue
        names = directives(entry.children, "server_name")
        listeners = directives(entry.children, "listen")
        tls = any(row and row[0].rsplit(":", 1)[-1] == "443" and "ssl" in row
                  for row in listeners)
        if tls and any(HOST in row for row in names):
            if names != [(HOST,)]:
                raise edge.EdgeError("shared TLS vhost requires operator review")
            servers.append(entry)
    if len(servers) != 1:
        raise edge.EdgeError("exactly one dedicated Pandaolate TLS server required")
    server = servers[0]
    locations = [e for e in server.children
                 if e.words == ("location", "/") and e.opening is not None]
    if len(locations) != 1:
        raise edge.EdgeError("exactly one public location / required")
    location = locations[0]
    if directives(location.children, "proxy_pass") not in (
        [("http://127.0.0.1:3000",)], [("http://127.0.0.1:3000/",)]
    ):
        raise edge.EdgeError("unsupported public proxy upstream")
    for scope in (server.children, location.children):
        if directives(scope, "error_page"):
            raise edge.EdgeError("existing error_page requires review")
    if directives(location.children, "proxy_intercept_errors"):
        raise edge.EdgeError("existing proxy_intercept_errors requires review")
    if any(row != ("off",) for row in directives(server.children, "proxy_intercept_errors")):
        raise edge.EdgeError("existing proxy interception requires review")
    prefix = str(root / "current")
    insertions = {
        server.start: f"include {prefix}/nginx-http.conf;\n\n",
        server.opening: f"\n    include {prefix}/nginx-server.conf;",
        location.opening: f"\n        include {prefix}/nginx-proxy.conf;",
    }
    for index, insertion in sorted(insertions.items(), reverse=True):
        text = text[:index] + insertion + text[index:]
    return text.encode("utf-8")


@dataclass(frozen=True)
class Snapshot:
    status: int
    headers: dict[str, tuple[str, ...]]

    def hsts(self):
        values = self.headers.get("strict-transport-security", ())
        if len(values) > 1:
            raise edge.EdgeError("ambiguous live HSTS policy")
        return edge.validate_hsts(values[0]) if values else None


def compare_catalog(before: Snapshot, after: Snapshot):
    if before.status != after.status or before.hsts() != after.hsts():
        raise edge.EdgeError("catalog HTTP status or HSTS changed")
    # For an error page, its own stricter CSP/referrer policy is intentional.
    if before.status < 500 and any(
        before.headers.get(name, ()) != after.headers.get(name, ()) for name in SECURITY
    ):
        raise edge.EdgeError("normal catalog security headers changed")


def hash_bytes(data):
    return hashlib.sha256(data).hexdigest()


def checked_vhost(path: Path):
    if not path.is_absolute() or path.resolve() != path:
        raise edge.EdgeError("vhost must be a canonical regular file")
    edge.trusted(path.parent, directory=True)
    info = edge.trusted(path)
    if info.st_nlink != 1 or not 0 < info.st_size <= MAX_CONFIG:
        raise edge.EdgeError("unsupported vhost file type or size")
    return info, edge.read_small(path, MAX_CONFIG)


def atomic_replace(path: Path, expected: bytes, data: bytes, info):
    current, content = checked_vhost(path)
    if content != expected or (current.st_mode, current.st_gid) != (info.st_mode, info.st_gid):
        raise edge.EdgeError("vhost changed during adoption")
    fd, name = tempfile.mkstemp(prefix=".tor-edge-", dir=path.parent)
    temporary = Path(name)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            if os.fstat(stream.fileno()).st_gid != info.st_gid:
                os.fchown(stream.fileno(), -1, info.st_gid)
            os.fchmod(stream.fileno(), stat.S_IMODE(info.st_mode))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        edge.sync_dir(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def backup_original(store, original):
    directory = store.root / "adoption-backups"
    directory.mkdir(mode=0o700, exist_ok=True)
    if stat.S_IMODE(edge.trusted(directory, directory=True).st_mode) != 0o700:
        raise edge.EdgeError("adoption backup directory must be private")
    backup = directory / (hash_bytes(original) + ".conf")
    if os.path.lexists(backup):
        if (edge.read_small(backup, MAX_CONFIG) != original
                or stat.S_IMODE(edge.trusted(backup).st_mode) != 0o600):
            raise edge.EdgeError("existing adoption backup differs or is not private")
    else:
        fd = os.open(backup, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(original)
            os.fchmod(stream.fileno(), 0o600)
            stream.flush()
            os.fsync(stream.fileno())
        edge.sync_dir(directory)
    return backup


def adopt(store, source, sha, signers, vhost, approved_hash, nginx):
    """Caller holds overall release and edge locks. No app stop or DB access."""
    info, original = checked_vhost(vhost)
    if not re.fullmatch(r"[0-9a-f]{64}", approved_hash) or hash_bytes(original) != approved_hash:
        raise edge.EdgeError("vhost differs from approved SHA-256")
    if os.path.lexists(store.current):
        raise edge.EdgeError("edge already adopted or incomplete adoption requires recovery")
    candidate = render(original, store.root)
    nginx.test()
    nginx.loaded(vhost)
    before = nginx.snapshot("/catalog")
    old_asset = nginx.snapshot("/_tor-fallback/game.js")
    if old_asset.headers.get(edge.MARKER.lower()):
        raise edge.EdgeError("existing live edge marker requires review")
    store.preserve_hsts(before.hsts())
    target = store.prepare(source, sha, signers)
    backup = backup_original(store, original)
    try:
        store.switch_link(sha)
        atomic_replace(vhost, original, candidate, info)
        nginx.test()
        nginx.reload()
        nginx.probe(sha, target)
        compare_catalog(before, nginx.snapshot("/catalog"))
        if nginx.snapshot("/_tor-fallback/game.js").hsts() != before.hsts():
            raise edge.EdgeError("fallback asset does not preserve live HSTS")
    except (Exception, KeyboardInterrupt) as failure:
        try:
            _, actual = checked_vhost(vhost)
            if actual == candidate:
                atomic_replace(vhost, candidate, original, info)
            elif actual != original:
                raise edge.EdgeError("vhost changed externally; refusing overwrite")
            nginx.test()
            nginx.reload()
            # Reload is asynchronous: old workers may still answer briefly.
            for attempt in range(8):
                try:
                    compare_catalog(before, nginx.snapshot("/catalog"))
                    restored = nginx.snapshot("/_tor-fallback/game.js")
                    if restored.status != old_asset.status or restored.headers.get(edge.MARKER.lower()):
                        raise edge.EdgeError("previous HTTPS response not restored")
                    break
                except (edge.EdgeError, OSError, subprocess.SubprocessError):
                    if attempt == 7:
                        raise
                    time.sleep(0.5)
            if os.path.lexists(store.current):
                if not store.current.is_symlink() or os.readlink(store.current) != "releases/" + sha:
                    raise edge.EdgeError("managed link changed externally")
                store.current.unlink()
                edge.sync_dir(store.root)
        except (Exception, KeyboardInterrupt):
            raise edge.EdgeError("ADOPTION_ROLLBACK_UNVERIFIED: operator recovery required") from None
        raise edge.EdgeError("ADOPTION_ROLLBACK_VERIFIED: adoption refused") from failure
    return backup


class ProductionAdoption(edge.ProductionNginx):
    def loaded(self, path):
        result = subprocess.run(self.nginx_command("-T"), capture_output=True, check=True, timeout=20)
        # Never print nginx -T; it can contain credentials and private paths.
        config = result.stdout.decode("utf-8")
        markers = re.findall(r"^# configuration file (/.+):$", config, re.M)
        if sum(Path(name).resolve() == path for name in markers) != 1:
            raise edge.EdgeError("approved file not loaded exactly once")
        active = "\n".join(line for line in config.splitlines()
                           if not line.lstrip().startswith("#"))
        if ("/_tor-fallback/" in active or "tor-edge/current/" in active
                or re.search(r"\berror_page\s", active)
                or re.search(r"\bproxy_intercept_errors\s+on\s*;", active)):
            raise edge.EdgeError("existing active error handling requires review")

    def snapshot(self, route):
        if route not in ("/catalog", "/_tor-fallback/game.js"):
            raise edge.EdgeError("unsupported probe path")
        result = subprocess.run([
            "curl", "--disable", "--silent", "--show-error", "--http1.1",
            "--max-time", "5", "--max-filesize", "2097152", "--noproxy", "*",
            "--resolve", HOST + ":443:127.0.0.1", "--output", "/dev/null",
            "--dump-header", "-", "https://" + HOST + route,
        ], capture_output=True, check=True, timeout=7)
        block = result.stdout.rstrip(b"\r\n").split(b"\r\n\r\n")[-1]
        first, *lines = block.split(b"\r\n")
        status = re.match(rb"HTTP/1\.[01] ([1-5][0-9]{2})(?: |$)", first)
        if not status:
            raise edge.EdgeError("invalid HTTPS status")
        headers = {}
        for line in lines:
            key, sep, val = line.partition(b":")
            if not sep:
                raise edge.EdgeError("unsupported HTTPS header")
            key = key.decode("ascii").lower()
            if key in (*SECURITY, edge.MARKER.lower()):
                headers.setdefault(key, []).append(val.decode("ascii").strip())
        return Snapshot(int(status.group(1)), {k: tuple(v) for k, v in headers.items()})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("plan", "adopt"))
    parser.add_argument("--vhost", type=Path, required=True)
    parser.add_argument("--sha")
    parser.add_argument("--expected-vhost-sha256")
    args = parser.parse_args()
    try:
        if os.geteuid() != 0:
            raise edge.EdgeError("root privileges required")
        if args.vhost.parent not in (Path("/etc/nginx/sites-available"), Path("/etc/nginx/conf.d")):
            raise edge.EdgeError("unsupported vhost directory")
        for parent in (Path("/etc"), Path("/etc/nginx"), args.vhost.parent):
            edge.trusted(parent, directory=True)
        nginx = ProductionAdoption()
        if args.action == "plan":
            _, original = checked_vhost(args.vhost)
            render(original, edge.ROOT)
            nginx.test()
            nginx.loaded(args.vhost)
            print("TOR_EDGE_ADOPTION_PLAN_SHA256 " + hash_bytes(original))
            return 0
        if args.sha is None or args.expected_vhost_sha256 is None:
            raise edge.EdgeError("explicit approved vhost hash and full release SHA required")
        bundle.validate_sha(args.sha)
        store = edge.EdgeStore(edge.ROOT)
        # Same lock as existing one-click root executor; never modify concurrently.
        import fcntl
        fd = os.open("/run/lock/tor-one-click-release.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o077:
                raise edge.EdgeError("unsafe release lock")
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with store.locked():
                adopt(store, edge.RELEASES / args.sha, args.sha, edge.SIGNERS,
                      args.vhost, args.expected_vhost_sha256, nginx)
        finally:
            os.close(fd)
        print("TOR_EDGE_ADOPTED " + args.sha)
        return 0
    except (edge.EdgeError, bundle.BundleError, OSError, UnicodeError, subprocess.SubprocessError) as exc:
        detail = str(exc) if isinstance(exc, edge.EdgeError) else type(exc).__name__
        print("TOR_EDGE_ADOPTION_REFUSED: " + detail, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
