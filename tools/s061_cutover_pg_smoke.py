#!/usr/bin/env python3
"""S061: real isolated Rust/PostgreSQL candidate, cutover and rollback drill.

Never contacts a VPS, external host, or non-disposable database. The two
release directories contain the same tested TOR binary; this proves release
mechanics, not compatibility between two independently built versions.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

EXPECTED = {
    "GITHUB_ACTIONS": "true",
    "PGHOST": "127.0.0.1",
    "PGUSER": "postgres",
    "PGDATABASE": "postgres",
    "TEST_DATABASE_URL": "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
    "MIGRATOR_URL": "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci",
}
LIVE_PORT = 3091
CANDIDATE_PORT = 3092
BAD_PORT = 3093
PUBLIC_TITLE = "TOR S061 Surviving Book"
PRIVATE_TITLE = "TOR S061 Private Book"


def require_disposable(environment: dict[str, str]) -> Path:
    """Refuse every unknown host, role, database, or non-GitHub execution."""
    if any(environment.get(key) != value for key, value in EXPECTED.items()):
        raise RuntimeError("S061 refuses any non-disposable GitHub Actions PostgreSQL")
    temp = Path(environment.get("RUNNER_TEMP", ""))
    if not temp.is_absolute() or not temp.is_dir() or temp.is_symlink():
        raise RuntimeError("S061 requires a real absolute GitHub runner temp directory")
    return temp


def select_previous_binary(environment: dict[str, str], runner_temp: Path,
                           candidate_binary: Path) -> tuple[Path, bool]:
    """Opt-in: previous executable must be an independent runner-local build."""
    value = environment.get("TOR_S061_PREVIOUS_BINARY", "")
    if not value:
        return candidate_binary, False
    previous = Path(value)
    if (not previous.is_absolute() or previous.is_symlink()
            or not previous.is_file() or not os.access(previous, os.X_OK)
            or not previous.is_relative_to(runner_temp.resolve())):
        raise RuntimeError("S061 previous binary must be an executable in runner temp")
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    if digest(previous) == digest(candidate_binary):
        raise RuntimeError("S061 distinct-binary drill refuses identical binaries")
    return previous, True


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def sql(text: str) -> str:
    result = subprocess.run(
        ["psql", EXPECTED["TEST_DATABASE_URL"], "-X", "-A", "-t",
         "-v", "ON_ERROR_STOP=1"],
        input=text, text=True, capture_output=True, timeout=25, check=True,
    )
    return result.stdout.strip()


def migration_state() -> str:
    result = sql(
        "SELECT count(*) FILTER (WHERE success = TRUE) || ':' || "
        "coalesce(max(version) FILTER (WHERE success = TRUE), 0) || ':' || "
        "count(*) FILTER (WHERE success = FALSE) FROM _sqlx_migrations;"
    )
    require(re.fullmatch(r"[1-9][0-9]*:[0-9]+:0", result) is not None,
            "S061 requires completed SQLx migrations without failed records")
    return result


def seed_books() -> tuple[int, int]:
    sql(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)"
        " VALUES ('ci:s061:public','ru','TOR S061 Surviving Book','public','basic')"
        " ON CONFLICT (source_identity,target_language) DO NOTHING;"
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)"
        " VALUES ('ci:s061:private','ru','TOR S061 Private Book','draft','basic')"
        " ON CONFLICT (source_identity,target_language) DO NOTHING;"
    )
    public = sql(
        "SELECT id FROM books WHERE source_identity='ci:s061:public' "
        "AND target_language='ru' AND visibility='public';"
    )
    private = sql(
        "SELECT id FROM books WHERE source_identity='ci:s061:private' "
        "AND target_language='ru' AND visibility='draft';"
    )
    require(public.isdigit() and private.isdigit() and public != private,
            "S061 database fixtures are missing or incorrectly visible")
    return int(public), int(private)


def http(port: int, route: str) -> tuple[int, str]:
    url = f"http://127.0.0.1:{port}{route}"
    try:
        response = urllib.request.urlopen(url, timeout=1.5)
    except urllib.error.HTTPError as error:
        response = error
    except (urllib.error.URLError, TimeoutError, OSError):
        return 0, ""
    with response:
        return response.status, response.read(2_000_000).decode("utf-8", "replace")


def is_ready(port: int) -> bool:
    return http(port, "/readyz") == (200, "ok")


def wait_ready(process: subprocess.Popen, port: int) -> None:
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"S061 Rust server exited before readiness on {port}")
        if is_ready(port):
            return
        time.sleep(0.2)
    raise RuntimeError(f"S061 PostgreSQL readiness timed out on {port}")


def assert_http(port: int, public_id: int, private_id: int) -> None:
    require(http(port, "/healthz") == (200, "ok"), "health gate failed")
    require(is_ready(port), "PostgreSQL readiness gate failed")
    require(http(port, "/catalog")[0] == 200, "public catalog smoke failed")
    status, registration = http(port, "/register")
    require(status == 200 and 'name="csrf_token"' in registration,
            "authentication/CSRF smoke failed")
    status, public = http(port, f"/books/{public_id}")
    require(status == 200 and PUBLIC_TITLE in public, "durable book not visible")
    require(http(port, f"/books/{private_id}")[0] == 404,
            "private draft exposed during release transition")
    require(PRIVATE_TITLE not in http(port, "/api/v1/books")[1],
            "private draft leaked into public catalog")


def atomic_select(current: Path, target: Path) -> None:
    """Atomically replace only the private release symlink, never a directory."""
    require(target.parent == current.parent / "releases"
            and target.is_dir() and not target.is_symlink(),
            "invalid candidate release directory")
    require(current.is_symlink(), "current release must be a symlink")
    replacement = current.parent / f".current-{os.getpid()}"
    require(not replacement.exists() and not replacement.is_symlink(),
            "leftover release switch marker")
    replacement.symlink_to(target)
    os.replace(replacement, current)


def stop(process: subprocess.Popen | None, force: bool = False) -> None:
    if process is None or process.poll() is not None:
        return
    if force:
        process.kill()
    else:
        process.terminate()
    try:
        process.wait(timeout=6)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=6)


def no_listener(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as connection:
        connection.settimeout(0.2)
        return connection.connect_ex(("127.0.0.1", port)) != 0


def run() -> None:
    runner_temp = require_disposable(dict(os.environ))
    binary = Path("target/debug/tor-web").resolve()
    require(binary.is_file() and os.access(binary, os.X_OK),
            "S061 requires the previously built Rust server")
    previous_binary, distinct = select_previous_binary(dict(os.environ), runner_temp, binary)
    require(all(no_listener(port) for port in (LIVE_PORT, CANDIDATE_PORT, BAD_PORT)),
            "S061 loopback port already in use")
    before = migration_state()
    public_id, private_id = seed_books()
    output = runner_temp / "s061-real-cutover"
    output.mkdir(mode=0o700, exist_ok=False)
    processes: list[subprocess.Popen] = []
    logs = []
    events = []

    with tempfile.TemporaryDirectory(prefix="s061-release-", dir=runner_temp) as directory:
        root = Path(directory)
        releases = root / "releases"
        previous = releases / ("a" * 40)
        candidate = releases / ("b" * 40)
        previous.mkdir(parents=True)
        candidate.mkdir()
        for target, executable in ((previous, previous_binary), (candidate, binary)):
            path = target / "tor-web"
            shutil.copy2(executable, path)
            path.chmod(0o700)
        current = root / "current"
        current.symlink_to(previous)

        def launch(path: Path, port: int, name: str, bad_db: bool = False):
            database = EXPECTED["TEST_DATABASE_URL"]
            if bad_db:
                database = database.replace(":5432/", ":5433/")
            env = dict(os.environ, DATABASE_URL=database, TOR_DEMO_MODE="0",
                       TOR_AUTH_ENABLED="1", TOR_COVER_WORKER_ENABLED="0",
                       TOR_LISTEN_ADDR=f"127.0.0.1:{port}",
                       TOR_PUBLIC_ORIGIN=f"http://127.0.0.1:{port}")
            logfile = (root / f"{name}.log").open("w", encoding="utf-8")
            logs.append(logfile)
            process = subprocess.Popen([str(path)], env=env, stdout=logfile,
                                       stderr=subprocess.STDOUT)
            processes.append(process)
            return process

        try:
            old = launch(current / "tor-web", LIVE_PORT, "previous")
            wait_ready(old, LIVE_PORT)
            assert_http(LIVE_PORT, public_id, private_id)
            events.append("previous_live")

            # A broken candidate cannot switch the active release.
            bad = launch(candidate / "tor-web", BAD_PORT, "invalid-db", bad_db=True)
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline and bad.poll() is None:
                require(not is_ready(BAD_PORT),
                        "candidate accepted a broken PostgreSQL connection")
                time.sleep(0.2)
            stop(bad)
            require(current.resolve() == previous and is_ready(LIVE_PORT),
                    "failed candidate replaced or broke the previous release")
            events.append("bad_candidate_rejected")

            # Run the actual candidate alongside the live TOR process.
            trial = launch(candidate / "tor-web", CANDIDATE_PORT, "candidate")
            wait_ready(trial, CANDIDATE_PORT)
            assert_http(CANDIDATE_PORT, public_id, private_id)
            require(is_ready(LIVE_PORT) and current.resolve() == previous,
                    "candidate inspection disrupted existing traffic")
            events.append("parallel_candidate_smoke_passed")

            # Mirror the current single-port systemd release: atomic symlink
            # cutover followed by a restart. This is not zero-downtime routing.
            stop(trial)
            atomic_select(current, candidate)
            stop(old)
            promoted = launch(current / "tor-web", LIVE_PORT, "promoted")
            wait_ready(promoted, LIVE_PORT)
            assert_http(LIVE_PORT, public_id, private_id)
            require(current.resolve() == candidate, "candidate was not activated")
            events.append("atomic_cutover_ready")

            # Force a real post-switch process failure and restore the old binary.
            stop(promoted, force=True)
            require(not is_ready(LIVE_PORT), "killed candidate still reported ready")
            atomic_select(current, previous)
            recovered = launch(current / "tor-web", LIVE_PORT, "rolled-back")
            wait_ready(recovered, LIVE_PORT)
            assert_http(LIVE_PORT, public_id, private_id)
            require(current.resolve() == previous, "previous release not restored")
            events.append("rollback_after_crash_verified")
            require(migration_state() == before,
                    "SQLx migration history changed during version switching")
            events.append("postgresql_and_acl_survived")
            report = {
                "scenario": "S061",
                "checked_out_sha": os.environ.get("GITHUB_SHA", ""),
                "database": "disposable PostgreSQL 18 tor_ci",
                "separate_candidate": True,
                "precutover_rejection": True,
                "cutover": "atomic symlink plus process restart",
                "postcutover_rollback": True,
                "same_binary_both_slots": not distinct,
                "distinct_binary_validation": distinct,
                "no_production_or_tls_staging_claim": True,
                "events": events,
            }
            (output / "report.json").write_text(
                json.dumps(report, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )
            print(json.dumps(report, ensure_ascii=False))
        finally:
            for process in reversed(processes):
                stop(process)
            for logfile in logs:
                logfile.close()


if __name__ == "__main__":
    run()
