#!/usr/bin/env python3
"""DEPLOY-02/03: install verified edge assets; switch and verify Nginx safely."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time
import uuid

import release_bundle_v2 as bundle

ROOT = Path("/var/lib/tor-edge")
RELEASES = Path("/opt/tor/releases")
SIGNERS = Path("/etc/tor/tor-release-allowed-signers")
ASSETS = {
    "index.html": "fallback-index.html",
    "style.css": "fallback-style.css",
    "game.js": "fallback-game.js",
    "plain.txt": "fallback-plain.txt",
}
SNIPPETS = ("nginx-http.conf", "nginx-server.conf", "nginx-proxy.conf")
CHANNEL = re.compile(r"https://t\.me/[A-Za-z][A-Za-z0-9_]{4,31}")
LINK = re.compile(r"releases/([0-9a-f]{40})")
PATH = re.compile(r"/[A-Za-z0-9_./-]+")
MARKER = "X-Tor-Edge-Release"
HSTS = re.compile(r"max-age=[0-9]{1,12}(?:;[ \t]*(?:includeSubDomains|preload)){0,2}", re.IGNORECASE)


def validate_hsts(value: str) -> str:
    if not HSTS.fullmatch(value):
        raise EdgeError("unsupported HSTS policy; operator review required")
    directives = [part.strip().lower() for part in value.split(";")[1:]]
    if len(set(directives)) != len(directives):
        raise EdgeError("duplicate HSTS directive")
    return value


class EdgeError(ValueError):
    """Fixed diagnostics; never print configuration or secrets."""


def trusted(path: Path, directory: bool = False):
    info = path.lstat()
    expected = stat.S_ISDIR if directory else stat.S_ISREG
    if (not expected(info.st_mode) or info.st_uid != os.geteuid()
            or info.st_mode & 0o022):
        raise EdgeError("unsafe owner, permissions or type: " + path.name)
    return info


def trusted_dir(path: Path):
    try:
        path.mkdir(mode=0o755)
    except FileExistsError:
        pass
    else:
        path.chmod(0o755)  # root release executor uses umask 077
    if not trusted(path, directory=True).st_mode & stat.S_IXOTH:
        raise EdgeError("managed directory not traversable by Nginx: " + path.name)


def settings_valid(data: bytes) -> bool:
    if len(data) > 4096:
        return False
    def unique_pairs(pairs):
        result = dict(pairs)
        if len(result) != len(pairs):
            raise ValueError("duplicate public setting")
        return result
    try:
        config = json.loads(data, object_pairs_hook=unique_pairs)
    except (ValueError, UnicodeError):
        return False
    if type(config) is not dict or set(config) != {"telegram_url"}:
        return False
    url = config["telegram_url"]
    return url is None or (type(url) is str and CHANNEL.fullmatch(url) is not None)


def read_small(path: Path, limit: int) -> bytes:
    info = trusted(path)
    if not 0 < info.st_size <= limit:
        raise EdgeError("unsafe file size: " + path.name)
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if not 0 < len(data) <= limit:
        raise EdgeError("unsafe file size: " + path.name)
    return data


def write_new(path: Path, data: bytes):
    with path.open("xb") as stream:
        stream.write(data)
        os.fchmod(stream.fileno(), 0o644)
        stream.flush()
        os.fsync(stream.fileno())


def sync_dir(path: Path):
    fd = os.open(path, os.O_DIRECTORY | os.O_RDONLY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class EdgeStore:
    def __init__(self, root: Path):
        if not PATH.fullmatch(str(root)) or root.resolve() != root:
            raise EdgeError("managed edge path must be canonical")
        self.root = root
        self.releases = root / "releases"
        self.settings = root / "settings"
        self.current = root / "current"

    @contextmanager
    def locked(self):
        trusted_dir(self.root)
        trusted_dir(self.releases)
        trusted_dir(self.settings)
        fd = os.open(self.root / ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
                raise EdgeError("unsafe edge lock")
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            yield
        finally:
            os.close(fd)

    def config(self, default: bytes):
        # Never overwrite operator configuration, including during rollback.
        if not settings_valid(default):
            raise EdgeError("invalid signed default settings")
        path = self.settings / "config.json"
        if not os.path.lexists(path):
            write_new(path, default)
            sync_dir(self.settings)
        if not settings_valid(read_small(path, 4096)):
            raise EdgeError("invalid operator settings")
        if not trusted(path).st_mode & stat.S_IROTH:
            raise EdgeError("operator settings must be readable by Nginx")

    def hsts_policy(self) -> str | None:
        path = self.settings / "hsts.txt"
        if not os.path.lexists(path):
            return None
        return validate_hsts(read_small(path, 256).decode("ascii"))

    def preserve_hsts(self, observed: str | None):
        """First adoption only; never silently replace an existing policy."""
        if observed is not None:
            validate_hsts(observed)
        if self.hsts_policy() is not None:
            if self.hsts_policy() != observed:
                raise EdgeError("stored HSTS differs from live policy")
        elif observed is not None:
            write_new(self.settings / "hsts.txt", observed.encode("ascii"))
            sync_dir(self.settings)

    def payload(self, source: Path, sha: str) -> dict[str, bytes]:
        site = self.releases / sha / "site"
        files = {"site/" + dest: read_small(source / src, bundle.LIMITS[src])
                 for dest, src in ASSETS.items()}
        for name in SNIPPETS:
            files[name] = read_small(source / name, bundle.LIMITS[name])
        # Retain the signed config's directives; render only the five exact
        # alias targets and a public version marker. Old workers keep immutable
        # release-specific paths while the new config is being tested/reloaded.
        text = files["nginx-server.conf"].decode("utf-8")
        for name in (*ASSETS, "config.json"):
            old = "alias /var/lib/tor-fallback/" + name + ";"
            target = self.settings / name if name == "config.json" else site / name
            if text.count(old) != 1:
                raise EdgeError("signed alias template does not match contract")
            text = text.replace(old, "alias " + str(target) + ";")
        if "/var/lib/tor-fallback/" in text:
            raise EdgeError("unexpected legacy alias")
        # Nginx 1.24 drops inherited add_header directives when a location
        # defines any of its own. Preserve the reviewed live HSTS policy.
        hsts = self.hsts_policy()
        if hsts is not None:
            if "strict-transport-security" in text.lower():
                raise EdgeError("signed template has a conflicting HSTS policy")
            for name in (*ASSETS, "config.json"):
                location = f"location = /_tor-fallback/{name} {{"
                if text.count(location) != 1:
                    raise EdgeError("unsupported HSTS location template")
                text = text.replace(location, location +
                                    f'\n    add_header Strict-Transport-Security "{hsts}" always;')
        marker = "location = /_tor-fallback/game.js {"
        if text.count(marker) != 1 or MARKER.lower() in text.lower():
            raise EdgeError("signed marker template does not match contract")
        text = text.replace(marker, marker + f'\n    add_header {MARKER} "{sha}" always;')
        files["nginx-server.conf"] = text.encode("utf-8")
        # Record the source manifest for deterministic reuse and local auditing.
        files["source-manifest"] = read_small(source / bundle.MANIFEST, 4096)
        files["edge-checksums.json"] = self.checksums(files)
        return files

    @staticmethod
    def checksums(files: dict[str, bytes]) -> bytes:
        hashes = {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}
        return (json.dumps(hashes, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")

    def inspect(self, sha: str) -> Path:
        bundle.validate_sha(sha)
        directory = self.releases / sha
        trusted(directory, directory=True)
        trusted(directory / "site", directory=True)
        expected = {*SNIPPETS, "site", "source-manifest", "edge-checksums.json"}
        if {p.name for p in directory.iterdir()} != expected:
            raise EdgeError("managed release layout mismatch")
        if {p.name for p in (directory / "site").iterdir()} != set(ASSETS):
            raise EdgeError("managed site layout mismatch")
        for path in (directory, directory / "site"):
            if not trusted(path, directory=True).st_mode & stat.S_IXOTH:
                raise EdgeError("managed release not traversable by Nginx")
        files = {rel: read_small(directory / rel, 2 * 1024 * 1024)
                 for rel in (*SNIPPETS, "source-manifest", *("site/" + x for x in ASSETS))}
        if read_small(directory / "edge-checksums.json", 4096) != self.checksums(files):
            raise EdgeError("managed release checksum mismatch")
        manifest = files["source-manifest"]
        if not manifest.startswith(f"TOR_RELEASE_MANIFEST 2\nTOR_RELEASE_SHA {sha}\n".encode()):
            raise EdgeError("managed release identity mismatch")
        return directory

    def prepare(self, source: Path, sha: str, signers: Path) -> Path:
        """Requires root-owned, already staged v2 directory; no live change."""
        bundle.validate_sha(sha)
        trusted(source, directory=True)
        trusted(signers)
        for name in bundle.FILES:
            trusted(source / name)
        bundle.verify(source, sha, signers)
        files = self.payload(source, sha)
        default = read_small(source / "fallback-config.example.json", 4096)
        target = self.releases / sha
        if os.path.lexists(target):
            self.inspect(sha)
            for name, data in files.items():
                if read_small(target / name, 2 * 1024 * 1024) != data:
                    raise EdgeError("existing managed release differs from signed candidate")
        else:
            with bundle.candidate(target) as tmp:
                trusted_dir(tmp / "site")
                for name, data in files.items():
                    write_new(tmp / name, data)
                os.chmod(tmp, 0o755)
                sync_dir(tmp / "site")
                sync_dir(tmp)
            sync_dir(self.releases)
        self.config(default)
        return target

    def active_sha(self) -> str:
        if not self.current.is_symlink():
            raise EdgeError("one-time managed Nginx adoption required")
        if self.current.lstat().st_uid != os.geteuid():
            raise EdgeError("current link has unexpected owner")
        match = LINK.fullmatch(os.readlink(self.current))
        if not match:
            raise EdgeError("current link outside managed releases")
        sha = match.group(1)
        self.inspect(sha)
        return sha

    def switch_link(self, sha: str):
        self.inspect(sha)
        temporary = self.root / (".current-" + uuid.uuid4().hex)
        try:
            temporary.symlink_to("releases/" + sha)
            os.replace(temporary, self.current)
            sync_dir(self.root)
        finally:
            temporary.unlink(missing_ok=True)

    def activate(self, sha: str, nginx) -> str:
        """Requires an adopted vhost; restores the last known good edge on error."""
        self.inspect(sha)
        previous = self.active_sha()
        if not settings_valid(read_small(self.settings / "config.json", 4096)):
            raise EdgeError("invalid operator settings")
        nginx.test()
        nginx.probe(previous, self.releases / previous)
        if previous == sha:
            return previous
        try:
            self.switch_link(sha)
            nginx.test()
            nginx.reload()
            nginx.probe(sha, self.releases / sha)
        except (Exception, KeyboardInterrupt) as failure:
            try:
                self.switch_link(previous)
                nginx.test()
                nginx.reload()
                nginx.probe(previous, self.releases / previous)
            except (Exception, KeyboardInterrupt):
                raise EdgeError("EDGE_ROLLBACK_UNVERIFIED: operator recovery required") from None
            raise EdgeError("EDGE_ROLLBACK_VERIFIED: candidate refused") from failure
        return previous


class ProductionNginx:
    """Fixed production commands; no caller-provided command or hostname."""

    @staticmethod
    def command(args):
        subprocess.run(args, check=True, timeout=20,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    def test(self):
        self.command(["nginx", "-t"])

    def reload(self):
        self.command(["systemctl", "reload", "nginx"])

    def probe(self, sha: str, directory: Path):
        expected = hashlib.sha256(read_small(directory / "site/game.js", 1024 * 1024)).digest()
        for _ in range(8):
            result = subprocess.run([
                "curl", "--disable", "--fail", "--silent", "--show-error", "--include",
                "--max-time", "4", "--max-filesize", "1048576",
                "--noproxy", "*", "--resolve", "pandaolate.ru:443:127.0.0.1",
                "https://pandaolate.ru/_tor-fallback/game.js",
            ], capture_output=True, timeout=6, check=False)
            headers, _, body = result.stdout.partition(b"\r\n\r\n")
            markers = [line.partition(b":")[2].strip()
                       for line in headers.splitlines()
                       if line.partition(b":")[0].lower() == MARKER.lower().encode()]
            if (result.returncode == 0 and markers == [sha.encode()]
                    and hashlib.sha256(body).digest() == expected):
                return
            time.sleep(0.5)
        raise EdgeError("active HTTPS edge marker or asset verification failed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "activate"))
    parser.add_argument("sha")
    args = parser.parse_args()
    try:
        if os.geteuid() != 0:
            raise EdgeError("root privileges required")
        bundle.validate_sha(args.sha)
        store = EdgeStore(ROOT)
        with store.locked():
            if args.action == "prepare":
                store.prepare(RELEASES / args.sha, args.sha, SIGNERS)
            else:
                previous = store.activate(args.sha, ProductionNginx())
                print("TOR_EDGE_PREVIOUS " + previous)
        print("TOR_EDGE_" + args.action.upper() + "_OK " + args.sha)
        return 0
    except (EdgeError, bundle.BundleError, OSError, UnicodeError,
            subprocess.SubprocessError) as error:
        detail = str(error) if isinstance(error, EdgeError) else type(error).__name__
        print("TOR_EDGE_REFUSED: " + detail, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
