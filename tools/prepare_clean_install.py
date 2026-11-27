#!/usr/bin/env python3
"""DEPLOY-06a: verify and prepare a private clean-install workspace only.

Never initialize PostgreSQL, install packages, start services, write live
configuration, create accounts, issue certificates or restore old data.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import stat
import sys

import managed_edge_release as edge
import release_bundle_v2 as bundle

PARENT = Path("/srv")
NAME = "tor-install"
HOST_ID = re.compile(r"[a-f0-9]{32}")
PIN = re.compile(r"[a-f0-9]{64}")
MARKERS = (
    "/opt/tor", "/etc/tor", "/var/lib/tor-release", "/var/lib/tor-edge",
    "/usr/local/lib/tor-release-v2", "/usr/local/sbin/tor-one-click-release",
    "/etc/systemd/system/tor-web.service",
    "/etc/systemd/system/tor-web.service.d",
    "/etc/systemd/system/tor-trace-store.service",
    "/etc/sudoers.d/tor-one-click-release",
    "/etc/nginx/sites-available/pandaolate",
    "/etc/nginx/sites-available/pandaolate.conf",
    "/etc/nginx/sites-enabled/pandaolate",
    "/etc/nginx/sites-enabled/pandaolate.conf",
    "/etc/nginx/conf.d/pandaolate.conf",
    "/srv/tor-recovery", "/var/lib/postgresql/18/main",
)
DESTINATIONS = {
    "install-nginx.conf": "/etc/nginx/sites-available/pandaolate.conf",
    "install-tor-web.service": "/etc/systemd/system/tor-web.service",
    "install-postgres.conf": "/etc/systemd/system/tor-web.service.d/20-postgres.conf",
    "systemd-web-release.conf": "/etc/systemd/system/tor-web.service.d/40-tor-release.conf",
}


class Refused(ValueError):
    pass


def validate_host_id(expected, actual):
    if (not isinstance(expected, str) or not HOST_ID.fullmatch(expected)
            or expected == "0" * 32 or expected != actual):
        raise Refused("not the independently approved clean-install host")


def absent(paths=MARKERS):
    if any(os.path.lexists(path) for path in paths):
        raise Refused("existing TOR, PostgreSQL or recovery marker; use UPDATE or RECOVER")


def check_parent(parent):
    if not parent.is_absolute() or parent.resolve(strict=True) != parent:
        raise Refused("workspace parent must be canonical")
    edge.trusted(parent, directory=True)
    if os.path.lexists(parent / NAME):
        raise Refused("install workspace already exists; inspect, never overwrite")


def verified_source(source, sha, signers, pin):
    bundle.validate_sha(sha)
    if not isinstance(pin, str) or not PIN.fullmatch(pin):
        raise Refused("independent manifest SHA-256 required")
    for path in (source, signers):
        if not path.is_absolute() or path.resolve(strict=True) != path:
            raise Refused("offline source/signers must be canonical")
    edge.trusted(source, directory=True)
    edge.trusted(signers)
    for name in bundle.FILES:
        edge.trusted(source / name)
    bundle.verify(source, sha, signers)
    manifest = edge.read_small(source / bundle.MANIFEST, 4096)
    if hashlib.sha256(manifest).hexdigest() != pin:
        raise Refused("signed manifest differs from independently approved pin")


def plan(sha, machine_id, pin):
    bundle.validate_sha(sha)
    validate_host_id(machine_id, machine_id)
    if not PIN.fullmatch(pin):
        raise Refused("invalid manifest pin")
    return {
        "format": 1,
        "mode": "INSTALL_EMPTY",
        "target_machine_id": machine_id,
        "release_sha": sha,
        "manifest_sha256": pin,
        "candidate_config_destinations": DESTINATIONS,
        "state": "prepared",
        "packages_installed": False,
        "database_initialized": False,
        "services_started": False,
        "dns_changed": False,
        "production_accepted": False,
    }


def prepare(parent, source, sha, signers, pin, machine_id):
    check_parent(parent)
    verified_source(source, sha, signers, pin)
    record = plan(sha, machine_id, pin)
    destination = parent / NAME
    with bundle.candidate(destination) as temporary:
        # All actual staged files stay underneath root-private 0700.
        # Stage copies then verifies, rather than trusting mutable source.
        release = temporary / "bundle"
        bundle.stage(source, release, sha, signers)
        verified_source(release, sha, signers, pin)
        release.chmod(0o700)
        config = temporary / "config"
        config.mkdir(mode=0o700)
        for name in DESTINATIONS:
            edge.write_new(config / name, edge.read_small(release / name, 1024 * 1024))
        edge.sync_dir(config)
        evidence = temporary / "install-plan.json"
        with evidence.open("xb") as out:
            os.fchmod(out.fileno(), 0o600)
            out.write((json.dumps(record, sort_keys=True, indent=2) + "\n").encode())
            out.flush()
            os.fsync(out.fileno())
        # The enclosing candidate helper fsyncs and publishes this directory.
        temporary.chmod(0o700)
    return destination


def check_host(expected):
    if os.geteuid() != 0 or "GITHUB_ACTIONS" in os.environ:
        raise Refused("run only as root on an approved independent clean VPS")
    os_release = platform.freedesktop_os_release()
    if os_release.get("ID") != "ubuntu" or os_release.get("VERSION_ID") != "24.04":
        raise Refused("expected Ubuntu 24.04 clean host")
    actual = Path("/etc/machine-id").read_text(encoding="ascii").strip()
    validate_host_id(expected, actual)
    absent()
    check_parent(PARENT)
    return actual


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-machine-id", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--signers", type=Path, required=True)
    parser.add_argument("--manifest-sha256", required=True)
    parser.add_argument("--prepare", action="store_true",
                        help="write private /srv/tor-install; default is read-only verification")
    args = parser.parse_args()
    try:
        os.umask(0o077)
        actual = check_host(args.target_machine_id)
        if args.prepare:
            prepare(PARENT, args.bundle, args.sha, args.signers,
                    args.manifest_sha256, actual)
        else:
            verified_source(args.bundle, args.sha, args.signers, args.manifest_sha256)
            plan(args.sha, actual, args.manifest_sha256)
        result = "PREPARED" if args.prepare else "CHECKED"
        print(f"TOR_CLEAN_INSTALL_{result} {args.sha} activation=false")
        return 0
    except (ValueError, OSError, KeyError, UnicodeError) as exc:
        detail = str(exc) if isinstance(exc, Refused) else type(exc).__name__
        print("TOR_CLEAN_INSTALL_REFUSED: " + detail, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
