#!/usr/bin/env python3
"""TOR Recovery: prepare a fresh, isolated local restore workspace.

Never starts services, installs packages, opens ports, reads cloud secrets,
runs migrations, restores data or activates a public TOR instance.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import platform
import pwd
import re
import shutil
import stat
import sys

PARENT = Path("/srv")
NAME = "tor-recovery"
PG_BIN = Path("/usr/lib/postgresql/18/bin")
HOST_ID = re.compile(r"[0-9a-f]{32}\Z")
LIVE_MARKERS = (
    "/opt/tor", "/etc/tor", "/var/lib/tor-release",
    "/etc/systemd/system/tor-web.service",
    "/etc/systemd/system/tor-web.service.d",
    "/usr/local/sbin/tor-one-click-release",
)
POSTGRES_DIRS = ("pgdata", "pgsocket", "pglog")
PRIVATE_DIRS = ("release", "objects", "evidence")


class Refused(ValueError):
    """Static, non-sensitive failure reason."""


def validate_target(source, expected, actual, port):
    for value in (source, expected, actual):
        if not isinstance(value, str) or not HOST_ID.fullmatch(value) or value == "0" * 32:
            raise Refused("valid independently recorded machine IDs required")
    if expected != actual:
        raise Refused("not the independently approved restore host")
    if source == actual:
        raise Refused("source host is not a restore destination")
    if type(port) is not int or not 15432 <= port <= 15999:
        raise Refused("isolated PostgreSQL port must be 15432-15999")


def reject_live_markers(paths=LIVE_MARKERS):
    if any(os.path.lexists(path) for path in paths):
        raise Refused("existing TOR installation or activation marker found")


def private_parent(parent):
    info = parent.lstat()
    if (not stat.S_ISDIR(info.st_mode)
            or parent.resolve(strict=True) != parent
            or info.st_uid != os.geteuid()
            or info.st_mode & 0o022):
        raise Refused("workspace parent must be trusted, owned and not writable by others")
    if os.path.lexists(parent / NAME):
        raise Refused("restore workspace already exists; never overwrite it")


def write_config(path, content, group):
    with path.open("x", encoding="utf-8") as handle:
        # The file is inside the still-private operator-owned workspace.
        handle.write(content)
    os.chown(path, os.geteuid(), group)
    path.chmod(0o640)


def prepare_layout(parent, postgres_uid, postgres_gid, port):
    if type(port) is not int or not 15432 <= port <= 15999:
        raise Refused("isolated PostgreSQL port must be 15432-15999")
    private_parent(parent)
    root = parent / NAME
    # Remain private until every directory and configuration is ready.
    root.mkdir(mode=0o700)
    for name in POSTGRES_DIRS:
        path = root / name
        path.mkdir(mode=0o700)
        os.chown(path, postgres_uid, postgres_gid)
        path.chmod(0o700)
    for name in PRIVATE_DIRS:
        (root / name).mkdir(mode=0o700)
    config = root / "config"
    config.mkdir(mode=0o750)
    os.chown(config, os.geteuid(), postgres_gid)
    config.chmod(0o750)

    # The actual PGDATA is intentionally EMPTY: no initdb before a physical
    # pgBackRest restore. No inherited production config or credential file.
    write_config(config / "postgresql.conf", f"""# Isolated recovery; do not use as production configuration.
data_directory = '{root}/pgdata'
hba_file = '{config}/pg_hba.conf'
ident_file = '{config}/pg_ident.conf'
listen_addresses = ''
port = {port}
unix_socket_directories = '{root}/pgsocket'
unix_socket_permissions = 0700
archive_mode = off
archive_command = ''
hot_standby = off
shared_preload_libraries = ''
""", postgres_gid)
    write_config(config / "pg_hba.conf",
                 "local all postgres peer\nlocal all all reject\n"
                 "host all all 0.0.0.0/0 reject\nhost all all ::/0 reject\n",
                 postgres_gid)
    write_config(config / "pg_ident.conf", "# No inherited role mappings.\n", postgres_gid)
    # postgres may traverse the workspace but cannot list its private files.
    os.chown(root, os.geteuid(), postgres_gid)
    root.chmod(0o710)
    return root


def check_host(source, target, port):
    if os.geteuid() != 0:
        raise Refused("root required on the approved independent recovery host")
    if "GITHUB_ACTIONS" in os.environ:
        raise Refused("operational recovery preparation is prohibited in CI")
    os_release = platform.freedesktop_os_release()
    if os_release.get("ID") != "ubuntu" or os_release.get("VERSION_ID") != "24.04":
        raise Refused("expected fresh Ubuntu 24.04 recovery host")
    machine_id = Path("/etc/machine-id").read_text(encoding="ascii").strip()
    validate_target(source, target, machine_id, port)
    reject_live_markers()
    private_parent(PARENT)
    postgres = pwd.getpwnam("postgres")
    if postgres.pw_uid == 0 or postgres.pw_gid == 0:
        raise Refused("unprivileged PostgreSQL OS account required")
    for name in ("postgres", "pg_ctl", "psql"):
        if not os.access(PG_BIN / name, os.X_OK):
            raise Refused("install trusted PostgreSQL 18 packages first")
    for name in ("pgbackrest", "rclone", "age", "ssh-keygen"):
        if not shutil.which(name):
            raise Refused("install trusted pgBackRest/rclone/age/OpenSSH packages first")
    return postgres


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-machine-id", required=True)
    parser.add_argument("--target-machine-id", required=True)
    parser.add_argument("--pg-port", type=int, default=15432)
    parser.add_argument("--prepare", action="store_true",
                        help="create the fixed private workspace; default checks only")
    args = parser.parse_args()
    try:
        postgres = check_host(args.source_machine_id, args.target_machine_id, args.pg_port)
        if args.prepare:
            prepare_layout(PARENT, postgres.pw_uid, postgres.pw_gid, args.pg_port)
    except (Refused, OSError, KeyError, UnicodeError) as error:
        reason = str(error) if isinstance(error, Refused) else "local host preparation failed"
        print("TOR_RECOVERY_HOST_REFUSED: " + reason, file=sys.stderr)
        return 2
    status = "PREPARED" if args.prepare else "CHECKED"
    print(f"TOR_RECOVERY_HOST_{status} port={args.pg_port} "
          "services_started=false recovery_accepted=false")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
