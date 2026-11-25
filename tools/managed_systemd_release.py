#!/usr/bin/env python3
"""DEPLOY-04: signed, narrow web service policy; never restart the service here."""
from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile

import managed_edge_release as trusted
import release_bundle_v2 as bundle

DROPIN_DIR = Path("/etc/systemd/system/tor-web.service.d")
NAME = "40-tor-release.conf"
SOURCE = "systemd-web-release.conf"
HEADER = "# TOR-managed release policy v1\n[Service]\n"
FIXED = {
    "Restart": "on-failure",
    "NoNewPrivileges": "yes",
    "ProtectSystem": "strict",
    "ProtectHome": "yes",
    "PrivateTmp": "yes",
    "PrivateDevices": "yes",
    "UMask": "0077",
}
# Preserve effective local operator configuration. Never read Environment or
# credentials; EnvironmentFiles contains paths, not contents.
PRESERVE = ("User", "Group", "WorkingDirectory", "FragmentPath",
            "EnvironmentFiles", "MemoryMax", "TasksMax", "CPUQuotaPerSecUSec",
            "ReadWritePaths", "ReadOnlyPaths", "InaccessiblePaths")
PROPERTIES = (*PRESERVE, *FIXED, "RestartUSec")


class UnitError(ValueError):
    pass


def policy(data: bytes) -> dict[str, str]:
    if not 0 < len(data) <= 4096:
        raise UnitError("managed unit policy size refused")
    try:
        text = data.decode("ascii")
    except UnicodeError:
        raise UnitError("managed unit policy must be ASCII") from None
    if not text.startswith(HEADER) or not text.endswith("\n"):
        raise UnitError("unrecognized managed unit policy")
    values = {}
    for line in text[len(HEADER):].splitlines():
        name, sep, value = line.partition("=")
        if (not sep or name not in (*FIXED, "RestartSec") or name in values
                or any(ord(char) < 32 for char in line)):
            raise UnitError("unsupported or duplicate managed directive")
        values[name] = value
    if set(values) != {*FIXED, "RestartSec"} or any(
            values[name] != val for name, val in FIXED.items()):
        raise UnitError("mandatory service isolation or restart policy changed")
    if not re.fullmatch(r"(?:[1-9]|[1-5][0-9]|60)s", values["RestartSec"]):
        raise UnitError("restart interval outside 1–60 seconds")
    return {**FIXED, "RestartUSec": values["RestartSec"]}


def read_current(directory: Path) -> bytes | None:
    trusted.trusted(directory, directory=True)
    path = directory / NAME
    if not os.path.lexists(path):
        return None
    info = trusted.trusted(path)
    if info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o644:
        raise UnitError("unsafe managed drop-in ownership, mode or links")
    data = trusted.read_small(path, 4096)
    policy(data)  # Do not overwrite an unknown operator file.
    return data


def replace(directory: Path, expected: bytes | None, data: bytes | None):
    if read_current(directory) != expected:
        raise UnitError("managed drop-in changed outside release")
    path = directory / NAME
    if data is None:
        if expected is not None:
            path.unlink()
    else:
        policy(data)
        fd, name = tempfile.mkstemp(prefix=".tor-unit-", dir=directory)
        temp = Path(name)
        try:
            with os.fdopen(fd, "wb") as stream:
                stream.write(data)
                os.fchmod(stream.fileno(), 0o644)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temp, path)
        finally:
            temp.unlink(missing_ok=True)
    trusted.sync_dir(directory)


class Systemd:
    """Production command boundary is fixed; no arbitrary service or arguments."""
    @staticmethod
    def command(*args):
        return subprocess.run(args, check=True, capture_output=True, timeout=25)

    def verify(self):
        self.command("systemd-analyze", "verify", "tor-web.service")

    def reload(self):
        self.command("systemctl", "daemon-reload")

    def snapshot(self):
        result = self.command("systemctl", "show", "tor-web.service",
                              *("--property=" + name for name in PROPERTIES))
        values = {}
        for line in result.stdout.decode("utf-8").splitlines():
            key, sep, value = line.partition("=")
            if not sep or key not in PROPERTIES or key in values:
                raise UnitError("unexpected systemd property response")
            values[key] = value
        if set(values) != set(PROPERTIES):
            raise UnitError("incomplete systemd property response")
        return values


def check_identity(snapshot):
    if (snapshot.get("User") != "tor-deploy" or
            snapshot.get("Group") != "tor-deploy" or
            snapshot.get("WorkingDirectory") != "/opt/tor" or
            not snapshot.get("FragmentPath")):
        raise UnitError("unexpected service identity")


def check_effective(before, after, wanted):
    check_identity(after)
    if any(after.get(name) != before.get(name) for name in PRESERVE):
        raise UnitError("operator-owned service properties changed")
    if any(after.get(name) != value for name, value in wanted.items()):
        raise UnitError("managed policy overridden or not loaded")


@dataclass(frozen=True)
class Receipt:
    previous: bytes | None
    candidate: bytes
    baseline: dict[str, str]


class UnitPolicy:
    """Root caller must hold the overall release lock for apply and restore."""
    def __init__(self, directory: Path = DROPIN_DIR):
        if not directory.is_absolute() or directory.resolve() != directory:
            raise UnitError("drop-in directory must be canonical")
        self.directory = directory

    def restore(self, receipt: Receipt, systemd):
        current = read_current(self.directory)
        if current not in (receipt.candidate, receipt.previous):
            raise UnitError("UNIT_ROLLBACK_UNVERIFIED: unexpected live drop-in")
        if current != receipt.previous:
            replace(self.directory, current, receipt.previous)
        systemd.verify()
        systemd.reload()
        if systemd.snapshot() != receipt.baseline:
            raise UnitError("UNIT_ROLLBACK_UNVERIFIED: effective policy differs")

    def apply(self, source: Path, sha: str, signers: Path, systemd) -> Receipt:
        """Verify signed staged bundle, apply policy; caller owns app restart."""
        trusted.trusted(source, directory=True)
        trusted.trusted(signers)
        for name in bundle.FILES:
            trusted.trusted(source / name)
        bundle.verify(source, sha, signers)
        data = trusted.read_small(source / SOURCE, 4096)
        wanted = policy(data)
        previous = read_current(self.directory)
        systemd.verify()
        baseline = systemd.snapshot()
        check_identity(baseline)
        receipt = Receipt(previous, data, baseline)
        if previous == data:
            check_effective(baseline, baseline, wanted)
            return receipt
        try:
            replace(self.directory, previous, data)
            systemd.verify()
            systemd.reload()
            check_effective(baseline, systemd.snapshot(), wanted)
        except (Exception, KeyboardInterrupt) as exc:
            try:
                self.restore(receipt, systemd)
            except (Exception, KeyboardInterrupt):
                raise UnitError("UNIT_ROLLBACK_UNVERIFIED: operator recovery required") from None
            raise UnitError("UNIT_ROLLBACK_VERIFIED: policy refused") from exc
        return receipt
