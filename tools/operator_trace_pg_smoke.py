#!/usr/bin/env python3
"""S059: real HTTP -> SQLx -> independent SQLite -> operator browser, CI only."""
import base64
import json
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


def request(path, password=None):
    headers = {"x-request-id": "CLIENT-SUPPLIED-TRACE-MUST-NOT-SURVIVE"}
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

    try:
        # Seed only this ephemeral database. No production fixture or migration changes.
        sql = """INSERT INTO books(source_identity,target_language,title,visibility,access_level)
                 SELECT 'ci:s059:'||n,'ru','S059 dataset '||n,'public','basic'
                 FROM generate_series(1,1000) n;"""
        seed = subprocess.run(["psql", database, "-X", "-q", "-v", "ON_ERROR_STOP=1"],
                              input=sql, text=True, capture_output=True, timeout=30)
        require(seed.returncode == 0, "disposable dataset setup failed")
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
        for phase, values in [("normal", normal), ("writer_down", down)]:
            require(percentile(values, .95) < P95_BUDGET_MS, phase + " p95 exceeded CI budget")
        report = {
            "scenario": "S059", "sha": os.environ.get("GITHUB_SHA"),
            "dataset_added_books": 1000, "samples_per_phase": SAMPLES,
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
