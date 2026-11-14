#!/usr/bin/env python3
"""S059: real HTTP -> SQLx -> independent SQLite -> operator browser, CI only."""
import base64
import json
import hashlib
import math
import os
from pathlib import Path
import re
import secrets
import selectors
import sqlite3
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = "http://127.0.0.1:3094"
SAMPLES = 40
# Regression ceiling for a debug binary on a shared CI runner, not a production SLO.
P95_BUDGET_MS = 2000


def require(value, message):
    if not value:
        raise RuntimeError(message)


def request(path, password=None, session=None):
    headers = {"x-request-id": "CLIENT-SUPPLIED-TRACE-MUST-NOT-SURVIVE"}
    if session is not None:
        headers["Cookie"] = "tor-dev-session=" + session
    if password:
        credential = base64.b64encode(f"operator:{password}".encode()).decode()
        headers["Authorization"] = "Basic " + credential
    start = time.monotonic()
    try:
        response = urllib.request.urlopen(
            urllib.request.Request(BASE + path, headers=headers), timeout=8
        )
    except urllib.error.HTTPError as error:
        response = error
    with response:
        body = response.read(2_000_000).decode()
        return response.status, response.headers, body, (time.monotonic() - start) * 1000


def wait_for(probe, message, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if probe():
            return
        time.sleep(0.1)
    raise RuntimeError(message)


def stop(process, force=False):
    if process and process.poll() is None:
        process.kill() if force else process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def percentile(values, fraction):
    require(bool(values), "empty latency sample")
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def main():
    database = os.environ.get("APP_URL", "")
    parsed = urllib.parse.urlsplit(database)
    require(os.environ.get("GITHUB_ACTIONS") == "true"
            and parsed.scheme == "postgresql" and parsed.hostname == "127.0.0.1"
            and parsed.port == 5432 and parsed.username == "tor_app"
            and parsed.path == "/tor_ci", "requires isolated GitHub Actions tor_ci")
    output = Path(os.environ["RUNNER_TEMP"]) / "s059-pg"
    output.mkdir(mode=0o700, exist_ok=False)
    private = output / "private"
    private.mkdir(mode=0o700)
    archive = private / "traces.sqlite3"
    socket = private / "traces.sock"
    password = secrets.token_urlsafe(36)
    processes, logs = [], []

    def launch(args, env=None, label="process"):
        log = open(output / (label + ".log"), "w")
        logs.append(log)
        process = subprocess.Popen(args, env=env, stdout=log, stderr=subprocess.STDOUT)
        processes.append(process)
        return process

    def writer(label):
        process = launch(["./target/debug/tor-trace-store", "serve", str(archive), str(socket)],
                         label=label)
        wait_for(lambda: socket.is_socket() and process.poll() is None,
                 "trace writer did not start")
        return process

    def stored(trace_id):
        if not archive.exists():
            return None
        with sqlite3.connect(f"file:{archive}?mode=ro", uri=True) as connection:
            return connection.execute(
                "SELECT duration_us,sqlx_known,queries,query_elapsed_us FROM traces WHERE trace_id=?",
                (trace_id,),
            ).fetchone()

    def catalog():
        code, headers, body, elapsed = request("/api/v1/books")
        require(code == 200 and isinstance(json.loads(body), list), "real catalog failed")
        trace_id = headers.get("x-request-id", "")
        require(re.fullmatch("[0-9a-f]{32}", trace_id), "server trace ID missing")
        return trace_id, elapsed

    def guest_book(book_id, session=None):
        # Fixed disposable public BASIC book. No identity or SQL text is reported.
        code, headers, body, elapsed = request(f"/books/{book_id}", session=session)
        require(code == 200 and "S059 dataset 1" in body,
                "isolated book detail did not render")
        if session is not None:
            require(f'/me/books/{book_id}/' in body
                    and "book-hero-actions__follow" in body,
                    "session was not authenticated for book controls")
        trace_id = headers.get("x-request-id", "")
        require(re.fullmatch("[0-9a-f]{32}", trace_id),
                "server book-detail trace ID missing")
        return trace_id, elapsed

    try:
        # Seed only this ephemeral database. No production fixture or migration changes.
        sql = """INSERT INTO books(source_identity,target_language,title,visibility,access_level)
                 SELECT 'ci:s059:'||n,'ru','S059 dataset '||n,'public','basic'
                 FROM generate_series(1,1000) n;"""
        seed = subprocess.run(["psql", database, "-X", "-q", "-v", "ON_ERROR_STOP=1"],
                              input=sql, text=True, capture_output=True, timeout=30)
        require(seed.returncode == 0, "disposable dataset setup failed")
        book_raw = subprocess.check_output(
            ["psql", database, "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-c",
             "SELECT id FROM books WHERE source_identity='ci:s059:1' LIMIT 1"],
            text=True, timeout=10).strip()
        require(book_raw.isdecimal() and int(book_raw) > 0,
                "isolated book fixture id missing")
        book_id = int(book_raw)
        store = writer("writer-before")
        env = dict(os.environ, DATABASE_URL=database, TOR_DEMO_MODE="0",
                   TOR_LISTEN_ADDR="127.0.0.1:3094", TOR_PUBLIC_ORIGIN=BASE,
                   TOR_OBS_OPERATOR_PASSWORD=password, TOR_TRACE_STORE_SOCKET=str(socket),
                   TOR_COVER_WORKER_ENABLED="0")
        server = launch(["./target/debug/tor-web"], env=env, label="web")

        def ready():
            require(server.poll() is None, "web process exited")
            try:
                return request("/readyz")[0] == 200
            except OSError:
                return False

        wait_for(ready, "real database readiness failed", 30)
        for _ in range(5):
            catalog()
        normal = [catalog()[1] for _ in range(SAMPLES)]

        # PERF-02a: reuse S059 SQLx -> SQLite trace evidence for the real
        # anonymous book page. The trace publisher samples ~1/16 requests;
        # select using only the independently generated server trace ID.
        # No route URL, book id, SQL text, token or body is persisted.
        for _ in range(3):
            guest_book(book_id)
        book_sql_samples = []
        book_attempts = 0
        while len(book_sql_samples) < 16 and book_attempts < 640:
            book_attempts += 1
            trace_id, http_ms = guest_book(book_id)
            if int(trace_id, 16) % 16:
                continue
            wait_for(lambda trace_id=trace_id: stored(trace_id) is not None,
                     "sampled guest book trace was not delivered")
            row = stored(trace_id)
            require(row[1] == 1 and row[2] >= 1 and row[3] >= 0,
                    "guest book SQLx attribution unavailable")
            book_sql_samples.append((row[2], row[3], http_ms))
        require(len(book_sql_samples) == 16,
                "insufficient sampled guest book SQLx traces")
        guest_book_baseline = {
            "route_family": "books",
            "mode": "anonymous_basic",
            "sampled_requests": len(book_sql_samples),
            "total_http_requests": book_attempts,
            "sqlx_known_fraction": 1.0,
            "sqlx_queries_p50": percentile([s[0] for s in book_sql_samples], .5),
            "sqlx_queries_p95": percentile([s[0] for s in book_sql_samples], .95),
            "sqlx_elapsed_us_p50": percentile([s[1] for s in book_sql_samples], .5),
            "sqlx_elapsed_us_p95": percentile([s[1] for s in book_sql_samples], .95),
            "http_elapsed_ms_p95": percentile([s[2] for s in book_sql_samples], .95),
            "scope": "HTTP task-local completed SQLx, 1/16 sampled traces; "
                     "debug isolated GitHub runner only, not PG server CPU",
        }

        # Synchronize on an actual acquired PostgreSQL lock, not a guessed sleep.
        lock = subprocess.Popen(["psql", database, "-X", "-qAt", "-v", "ON_ERROR_STOP=1"],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, text=True)
        processes.append(lock)
        lock.stdin.write("BEGIN; SET LOCAL statement_timeout='5s'; "
                         "LOCK TABLE books IN ACCESS EXCLUSIVE MODE;\n"
                         "\\echo S059_LOCK_ACQUIRED\nSELECT pg_sleep(1.5); COMMIT;\n")
        lock.stdin.close()
        with selectors.DefaultSelector() as selector:
            selector.register(lock.stdout, selectors.EVENT_READ)
            require(selector.select(timeout=8), "database lock was not acquired")
            require(lock.stdout.readline().strip() == "S059_LOCK_ACQUIRED", "lock setup failed")
        slow_id, slow_ms = catalog()
        lock.wait(timeout=8)
        require(lock.returncode == 0, "lock transaction failed")
        require(1000 <= slow_ms < 8000, "injected SQL delay was not observed")
        wait_for(lambda: stored(slow_id), "slow HTTP trace was not persisted")
        row = stored(slow_id)
        require(row[0] >= 1_000_000 and row[1] == 1 and row[2] >= 1
                and row[3] >= 1_000_000, "SQLx did not explain the injected database delay")
        with sqlite3.connect(f"file:{archive}?mode=ro", uri=True) as connection:
            spans = connection.execute(
                "SELECT completed_at_us,elapsed_us FROM query_spans WHERE trace_id=?", (slow_id,)
            ).fetchall()
        require(any(elapsed >= 1_000_000 for _, elapsed in spans), "slow SQL span missing")
        require(all(completed <= row[0] for completed, _ in spans), "span outside HTTP boundary")

        stop(store, force=True)
        down = [catalog()[1] for _ in range(SAMPLES)]
        code, headers, body, _ = request("/ops/traces?id=" + slow_id, password)
        require(code == 503 and headers.get("Cache-Control") == "no-store",
                "failed writer must be unavailable, not a false missing trace")
        # SIGKILL leaves the socket inode; only the disposable socket is removed.
        socket.unlink(missing_ok=True)
        writer("writer-after")
        require(stored(slow_id) == row, "restart lost or changed committed trace")
        fresh_id = None
        # A normal trace is sampled about once per 16 HTTP requests. A loop of 32
        # can fail by chance (~13%) even when delivery is healthy; allow enough
        # attempts to make the acceptance check stable without weakening it.
        for _ in range(192):
            candidate, _ = catalog()
            if int(candidate, 16) % 16 == 0:
                fresh_id = candidate
                break
        require(fresh_id is not None, "no sampled request after recovery")
        wait_for(lambda: stored(fresh_id), "trace delivery did not recover")
        # Fixed non-identifying FLOW evidence in the already isolated CI database.
        subprocess.run(["psql", database, "-X", "-q", "-v", "ON_ERROR_STOP=1", "-c",
                        "INSERT INTO ril_flow_hourly VALUES "
                        "(date_trunc('hour',now()),3,0,10,6,2) "
                        "ON CONFLICT (bucket,surface_code,acquisition_hint) DO UPDATE "
                        "SET html_requests=ril_flow_hourly.html_requests+10, "
                        "opted_in_html_requests=ril_flow_hourly.opted_in_html_requests+6, "
                        "observed_opted_in_visit_starts=ril_flow_hourly.observed_opted_in_visit_starts+2"],
                       check=True, timeout=10)
        browser_env = dict(os.environ, TOR_S059_ORIGIN=BASE, TOR_S059_TRACE_ID=slow_id,
                           TOR_TEST_OPERATOR_PASSWORD=password, TOR_S059_OUTPUT=str(output))
        subprocess.run(["node", "tools/operator_trace_pg_browser.cjs"], env=browser_env,
                       check=True, timeout=60)
        # PERF-02b: compare the exact same book page in the same auth-enabled
        # process for an anonymous guest and an actual server-verified session.
        # The session is synthetic, random, and stored only as its SHA-256 digest
        # in disposable tor_ci. The password cannot be used for a login.
        secret = secrets.token_urlsafe(32)
        session_hash = hashlib.sha256(secret.encode("ascii")).hexdigest()
        auth_fixture_sql = f"""WITH actor AS (
            INSERT INTO users(email_normalized,display_name,password_hash)
            VALUES ('s059-perf02-ci@example.invalid','S059 PERF02 Reader',
                    '$argon2id$ci-test-session-only-not-for-login')
            RETURNING id
        ) INSERT INTO sessions(user_id,token_hash,expires_at)
          SELECT id, decode('{session_hash}', 'hex'), now() + interval '1 hour'
          FROM actor;"""
        created = subprocess.run(
            ["psql", database, "-X", "-q", "-v", "ON_ERROR_STOP=1"],
            input=auth_fixture_sql, text=True, capture_output=True, timeout=15,
        )
        require(created.returncode == 0, "isolated authenticated fixture failed")
        stop(server)
        auth_env = dict(env, TOR_AUTH_ENABLED="1")
        server = launch(["./target/debug/tor-web"], env=auth_env, label="web-auth")
        wait_for(ready, "authenticated TOR server did not start", 30)

        def measure_book_mode(label, token):
            # Trace sampling is deterministic, based on server-issued IDs.
            # Record every request count, not merely the chosen 16 traces.
            for _ in range(3):
                guest_book(book_id, token)
            attempts, samples = 0, []
            while len(samples) < 16 and attempts < 640:
                attempts += 1
                trace_id, http_ms = guest_book(book_id, token)
                if int(trace_id, 16) % 16:
                    continue
                wait_for(lambda trace_id=trace_id: stored(trace_id) is not None,
                         "sampled book SQLx trace was not delivered")
                measurement = stored(trace_id)
                require(measurement[1] == 1 and measurement[2] >= 1
                        and measurement[3] >= 0,
                        "book SQLx observation was unknown")
                samples.append((measurement[2], measurement[3], http_ms))
            require(len(samples) == 16, "insufficient book SQLx measurements")
            return {
                "mode": label,
                "sampled_requests": len(samples),
                "total_http_requests": attempts,
                "sqlx_known_fraction_of_sampled": 1.0,
                "sqlx_queries_p50": percentile([x[0] for x in samples], .5),
                "sqlx_queries_p95": percentile([x[0] for x in samples], .95),
                "sqlx_elapsed_us_p50": percentile([x[1] for x in samples], .5),
                "sqlx_elapsed_us_p95": percentile([x[1] for x in samples], .95),
                "http_elapsed_ms_p95": percentile([x[2] for x in samples], .95),
            }

        auth_guest = measure_book_mode("guest_auth_enabled", None)
        auth_reader = measure_book_mode("authenticated_basic", secret)
        auth_comparison = {
            "route_family": "books",
            "auth_enabled": True,
            "same_book_same_process": True,
            "guest": auth_guest,
            "reader": auth_reader,
            "p50_query_delta_reader_minus_guest": (
                auth_reader["sqlx_queries_p50"] - auth_guest["sqlx_queries_p50"]
            ),
            "scope": "16 recorded sampled HTTP-task SQLx traces per mode; "
                     "actual cookie verified by book follow controls; "
                     "no active reader load or production CPU estimate",
        }
        secret = None

        for phase, values in [("normal", normal), ("writer_down", down)]:
            require(percentile(values, .95) < P95_BUDGET_MS, phase + " p95 exceeded CI budget")
        report = {
            "scenario": "S059+PERF02a", "sha": os.environ.get("GITHUB_SHA"),
            "dataset_added_books": 1000, "samples_per_phase": SAMPLES,
            "perf02_guest_book_sqlx_baseline": guest_book_baseline,
            "perf02_auth_enabled_book_comparison": auth_comparison,
            "debug_ci_p95_budget_ms": P95_BUDGET_MS,
            "normal_p95_ms": percentile(normal, .95),
            "writer_down_p95_ms": percentile(down, .95),
            "injected_http_ms": slow_ms, "injected_sqlx_us": row[3],
            "trace_id": slow_id, "restart_preserved_trace": True,
            "delivery_recovered": True,
            "scope": "HTTP handler and SQLx; asynchronous trace archive delivery. "
                     "No claim of background business-task timing or production SLO.",
        }
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))
    finally:
        for process in reversed(processes):
            stop(process)
        for log in logs:
            log.close()


if __name__ == "__main__":
    main()
