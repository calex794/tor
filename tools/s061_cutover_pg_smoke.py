#!/usr/bin/env python3
"""S061: real isolated Rust/PostgreSQL candidate, cutover and rollback drill.

Automatic CI exercises the release switch. The optional two-SHA run first
installs and populates the previous schema, then tests an upgrade and rollback.
"""
from __future__ import annotations

import hashlib
import json
import os
from contextlib import contextmanager, nullcontext
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
UPGRADE_DATABASE = "tor_test"
SQL_DIR = Path(__file__).resolve().parent


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
            or not previous.resolve().is_relative_to(runner_temp.resolve())):
        raise RuntimeError("S061 previous binary must be an executable in runner temp")
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    if digest(previous) == digest(candidate_binary):
        raise RuntimeError("S061 distinct-binary drill refuses identical binaries")
    return previous, True


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def sql(text: str, database: str) -> str:
    result = subprocess.run(
        ["psql", database, "-X", "-A", "-t", "-q",
         "-v", "ON_ERROR_STOP=1"],
        input=text, text=True, capture_output=True, timeout=25,
    )
    require(result.returncode == 0, f"S061 fixture SQL failed: {result.stderr.strip()}")
    return result.stdout.strip()


def migration_state(database: str) -> dict[int, str]:
    result = sql(
        "SELECT version, encode(checksum,'hex'), success FROM _sqlx_migrations ORDER BY version;",
        database,
    )
    entries = [row.split("|") for row in result.splitlines()]
    require(bool(entries) and all(re.fullmatch(r"[0-9]+\|[0-9a-f]+\|t", row)
                                  for row in result.splitlines()),
            "S061 requires successful SQLx migrations and their checksums")
    return {int(version): digest for version, digest, _ in entries}


def seed_books(database: str) -> tuple[int, int]:
    sql(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)"
        " VALUES ('ci:s061:public','ru','TOR S061 Surviving Book','public','basic')"
        " ON CONFLICT (source_identity,target_language) DO NOTHING;"
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)"
        " VALUES ('ci:s061:private','ru','TOR S061 Private Book','draft','basic')"
        " ON CONFLICT (source_identity,target_language) DO NOTHING;",
        database,
    )
    public = sql(
        "SELECT id FROM books WHERE source_identity='ci:s061:public' "
        "AND target_language='ru' AND visibility='public';", database,
    )
    private = sql(
        "SELECT id FROM books WHERE source_identity='ci:s061:private' "
        "AND target_language='ru' AND visibility='draft';", database,
    )
    require(public.isdigit() and private.isdigit() and public != private,
            "S061 database fixtures are missing or incorrectly visible")
    return int(public), int(private)


@contextmanager
def upgrade_database():
    """Create an empty database accepted by the real tor-migrate binary."""
    admin = "postgresql://postgres@127.0.0.1:5432/postgres"
    database = EXPECTED["TEST_DATABASE_URL"].replace("/tor_ci", f"/{UPGRADE_DATABASE}")
    migrator = EXPECTED["MIGRATOR_URL"].replace("/tor_ci", f"/{UPGRADE_DATABASE}")
    # A collision raises before entering the cleanup block. Never reuse or drop it.
    sql(f"CREATE DATABASE {UPGRADE_DATABASE} OWNER tor_migrator;", admin)
    try:
        sql(f"""
            REVOKE ALL ON DATABASE {UPGRADE_DATABASE} FROM PUBLIC;
            GRANT CONNECT ON DATABASE {UPGRADE_DATABASE} TO tor_app;
            REVOKE CREATE ON SCHEMA public FROM PUBLIC;
            GRANT USAGE ON SCHEMA public TO tor_app;
            ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
              GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
            ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
              GRANT USAGE,SELECT ON SEQUENCES TO tor_app;
        """, f"{admin.rsplit('/', 1)[0]}/{UPGRADE_DATABASE}")
        yield database, migrator
    finally:
        sql(f"DROP DATABASE {UPGRADE_DATABASE} WITH (FORCE);", admin)


def migrate(executable: Path, database: str) -> None:
    require(executable.is_file() and not executable.is_symlink() and
            os.access(executable, os.X_OK), "S061 requires the release's real tor-migrate binary")
    subprocess.run([str(executable)], env=dict(os.environ, DATABASE_URL=database),
                   timeout=180, check=True)


def catalog_snapshot(database: str) -> dict:
    return json.loads(sql((SQL_DIR / "s061_catalog_snapshot.sql").read_text(), database))


def assert_chapter(port: int, chapter: int) -> None:
    status, html = http(port, f"/reader/{chapter}")
    require(status == 200 and "S061 current free edition V2" in html,
            f"S061 public chapter did not return the current V2 (HTTP {status})")
    require("S061 historical free edition V1" not in html,
            "S061 public chapter exposed the historical V1")


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
    if distinct:
        require(re.fullmatch(r"[0-9a-f]{40}", os.environ.get("TOR_S061_PREVIOUS_SHA", ""))
                is not None, "S061 requires the previous source SHA for the upgrade report")
    require(all(no_listener(port) for port in (LIVE_PORT, CANDIDATE_PORT, BAD_PORT)),
            "S061 loopback port already in use")
    databases = upgrade_database() if distinct else nullcontext((EXPECTED["TEST_DATABASE_URL"], None))
    with databases as (database, migrator):
        exercise_releases(runner_temp, binary, previous_binary, database, migrator)


def exercise_releases(runner_temp: Path, binary: Path, previous_binary: Path,
                      database: str, migrator: str | None) -> None:
    distinct = migrator is not None
    if migrator is not None:
        migrate(runner_temp / "s061-previous-tor-migrate", migrator)
    before = migration_state(database)
    public_id, private_id = seed_books(database)
    chapter = None
    paid_chapter = None
    original_catalog = None
    if distinct:
        ids = sql((SQL_DIR / "s061_seed_populated_catalog.sql").read_text(), database)
        chapter, paid_chapter = map(int, ids.split("|"))
        original_catalog = catalog_snapshot(database)
        require(all(original_catalog.values()), "S061 catalog fixture must populate every check")

    def assert_release(port: int) -> None:
        assert_http(port, public_id, private_id)
        if chapter is not None:
            assert_chapter(port, chapter)
            status, html = http(port, f"/reader/{paid_chapter}")
            require(status in (200, 403, 404) and "S061 paid fixture" not in html,
                    "S061 could not verify anonymous paid-text denial")

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
            url = database.replace(":5432/", ":5433/") if bad_db else database
            env = dict(os.environ, DATABASE_URL=url, TOR_DEMO_MODE="0",
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
            assert_release(LIVE_PORT)
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

            upgraded = before
            if migrator is not None:
                migrate(Path("target/debug/tor-migrate").resolve(), migrator)
                upgraded = migration_state(database)
                require(before.items() <= upgraded.items(),
                        "candidate removed or modified installed SQLx migrations")
                require(catalog_snapshot(database) == original_catalog,
                        "persistent catalog changed during SQLx upgrade")
                assert_release(LIVE_PORT)
                events.append("populated_schema_upgraded_with_previous_live")

            # Run the actual candidate alongside the live TOR process.
            trial = launch(candidate / "tor-web", CANDIDATE_PORT, "candidate")
            wait_ready(trial, CANDIDATE_PORT)
            assert_release(CANDIDATE_PORT)
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
            assert_release(LIVE_PORT)
            require(current.resolve() == candidate, "candidate was not activated")
            if distinct:
                require(catalog_snapshot(database) == original_catalog,
                        "persistent catalog changed during cutover")
            events.append("atomic_cutover_ready")

            # Force a real post-switch process failure and restore the old binary.
            stop(promoted, force=True)
            require(not is_ready(LIVE_PORT), "killed candidate still reported ready")
            atomic_select(current, previous)
            recovered = launch(current / "tor-web", LIVE_PORT, "rolled-back")
            wait_ready(recovered, LIVE_PORT)
            assert_release(LIVE_PORT)
            require(current.resolve() == previous, "previous release not restored")
            events.append("rollback_after_crash_verified")
            require(migration_state(database) == upgraded,
                    "SQLx migration history changed during version switching")
            if distinct:
                require(catalog_snapshot(database) == original_catalog,
                        "persistent catalog changed during rollback")
                events.append("catalog_survived_on_upgraded_schema")
            events.append("postgresql_and_acl_survived")
            report = {
                "scenario": "S061",
                "checked_out_sha": os.environ.get("GITHUB_SHA", ""),
                "database": "disposable PostgreSQL 18 tor_test" if distinct else
                            "disposable PostgreSQL 18 tor_ci",
                "separate_candidate": True,
                "precutover_rejection": True,
                "cutover": "atomic symlink plus process restart",
                "postcutover_rollback": True,
                "same_binary_both_slots": not distinct,
                "distinct_binary_validation": distinct,
                "previous_source_sha": os.environ.get("TOR_S061_PREVIOUS_SHA", "") if distinct else "",
                "populated_catalog_verified": distinct,
                "added_migrations": sorted(upgraded.keys() - before.keys()),
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
