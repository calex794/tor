#!/usr/bin/env python3
"""TOR-STORE DROP-B05/B06: isolated PostgreSQL 18 + MinIO publication failures.

An executable *contract experiment*, not TOR production SQL/routes/rights.
Only connects to the disposable 127.0.0.1 tor_store_bench database and
127.0.0.1:9000 MinIO bucket. No user text, payments, tokens or migrations.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import time

import boto3
from botocore.config import Config
from botocore.exceptions import ClientError
import psycopg
from psycopg.rows import dict_row

from tor_store_seekable import (
    build_pack, extract_range, parse_manifest, parse_seek_table, sha256,
)

BUCKET = "tor-storage-disposable-bench"
PREFIX = "drop-fault-ci"
SCHEMA = "drop_fault_ci"
RESULTS = []
PROJECT_SQL = [
    "DROP SCHEMA IF EXISTS drop_fault_ci CASCADE",
    "CREATE SCHEMA drop_fault_ci",
    """CREATE TABLE drop_fault_ci.jobs (
        job_id text PRIMARY KEY, fingerprint text NOT NULL,
        owner text NOT NULL, lease_epoch bigint NOT NULL,
        lease_until timestamptz NOT NULL, state text NOT NULL
          CHECK (state IN ('reserved', 'verified', 'published', 'cancelled')),
        pack_key text, pack_sha text, manifest_key text, manifest_sha text
    )""",
    """CREATE TABLE drop_fault_ci.job_items (
        job_id text NOT NULL REFERENCES drop_fault_ci.jobs(job_id),
        edition_id text NOT NULL, chapter_id text NOT NULL,
        version_num integer NOT NULL, expected_prev text,
        raw_sha text NOT NULL, raw_bytes integer NOT NULL,
        PRIMARY KEY(job_id, edition_id)
    )""",
    """CREATE TABLE drop_fault_ci.editions (
        edition_id text PRIMARY KEY, chapter_id text NOT NULL,
        version_num integer NOT NULL, raw_sha text NOT NULL,
        pack_key text NOT NULL, manifest_key text NOT NULL,
        manifest_sha text NOT NULL,
        UNIQUE(chapter_id, version_num)
    )""",
    """CREATE TABLE drop_fault_ci.current_chapter (
        chapter_id text PRIMARY KEY,
        edition_id text NOT NULL REFERENCES drop_fault_ci.editions(edition_id)
    )""",
    """CREATE TABLE drop_fault_ci.purchased_pins (
        reader_id text NOT NULL,
        edition_id text NOT NULL REFERENCES drop_fault_ci.editions(edition_id),
        PRIMARY KEY(reader_id, edition_id)
    )""",
]


class Rejected(RuntimeError):
    """An unauthorized or incomplete publication must fail closed."""


class SimulatedCrash(RuntimeError):
    """A reproducible failure at a deterministic boundary."""


class StorageDown:
    def put_object(self, **kwargs):
        raise ConnectionError("simulated object store outage")

    def get_object(self, **kwargs):
        raise ConnectionError("simulated object store outage")


def assert_local_environment():
    url = os.environ["TOR_BENCH_PG_URL"]
    endpoint = os.environ["TOR_BENCH_S3_ENDPOINT"]
    if ("127.0.0.1:5432/tor_store_bench" not in url
            or "tor_bench:tor-bench-only@" not in url
            or endpoint != "http://127.0.0.1:9000"):
        raise Rejected("this experiment only runs against disposable CI resources")
    return url, endpoint


def connection(url):
    return psycopg.connect(url, autocommit=True, row_factory=dict_row,
                           connect_timeout=5)


def s3_client(endpoint):
    return boto3.client(
        "s3", endpoint_url=endpoint, region_name="us-east-1",
        aws_access_key_id=os.environ["TOR_BENCH_S3_USER"],
        aws_secret_access_key=os.environ["TOR_BENCH_S3_PASS"],
        config=Config(signature_version="s3v4",
                      max_pool_connections=16, connect_timeout=3,
                      read_timeout=20, retries={"total_max_attempts": 1},
                      s3={"addressing_style": "path"}))


def setup_database(conn):
    for statement in PROJECT_SQL:
        conn.execute(statement)


def specification(editions):
    """Batch item = (chapter, edition, version, expected_prev, raw bytes)."""
    return [{"chapter_id": chapter, "edition_id": edition,
             "version_num": version, "expected_prev": previous,
             "raw_sha": sha256(body), "raw_bytes": len(body)}
            for chapter, edition, version, previous, body in editions]


def fingerprint_of(items):
    canonical = json.dumps(items, ensure_ascii=False,
                           separators=(",", ":"), sort_keys=True).encode()
    return sha256(canonical)


def reserve(conn, job_id, owner, editions):
    items = specification(editions)
    fp = fingerprint_of(items)
    with conn.transaction():
        conn.execute(
            """INSERT INTO drop_fault_ci.jobs
               (job_id, fingerprint, owner, lease_epoch, lease_until, state)
               VALUES (%s, %s, %s, 1, clock_timestamp() + interval '5 minutes', 'reserved')""",
            (job_id, fp, owner))
        for item in items:
            conn.execute(
                """INSERT INTO drop_fault_ci.job_items
                   (job_id, edition_id, chapter_id, version_num,
                    expected_prev, raw_sha, raw_bytes)
                   VALUES (%s,%s,%s,%s,%s,%s,%s)""",
                (job_id, item["edition_id"], item["chapter_id"],
                 item["version_num"], item["expected_prev"],
                 item["raw_sha"], item["raw_bytes"]))
    return fp, 1


def gate(conn, job_id, owner, epoch, fingerprint, acceptable, lock=True):
    query = "SELECT * FROM drop_fault_ci.jobs WHERE job_id = %s"
    if lock:
        query += " FOR UPDATE"
    row = conn.execute(query, (job_id,)).fetchone()
    if row is None:
        raise Rejected("unknown job")
    if row["fingerprint"] != fingerprint:
        raise Rejected("approved content fingerprint mismatch")
    if row["owner"] != owner or row["lease_epoch"] != epoch:
        raise Rejected("stale worker lease")
    if row["lease_until"] <= datetime.now(timezone.utc):
        raise Rejected("expired worker lease")
    if row["state"] not in acceptable:
        raise Rejected("invalid job lifecycle state")
    return row


def immutable_put(client, key, content):
    """Never blindly replace an immutable key on retry."""
    try:
        client.put_object(Bucket=BUCKET, Key=key, Body=content, IfNoneMatch="*")
    except ClientError as exc:
        code = exc.response.get("Error", {}).get("Code", "")
        if code not in ("PreconditionFailed", "412"):
            raise
        # A repeat request is only allowed if the exact remote bytes match.
        found = client.get_object(Bucket=BUCKET, Key=key)["Body"].read()
        if sha256(found) != sha256(content) or len(found) != len(content):
            raise Rejected("immutable S3 key collision or corrupted remote bytes") from exc


def actual_pack_receipt(client, pack_key, manifest_key, expected_pack,
                        expected_manifest, editions):
    # Object store was read back; no trusting ETag or a previous local buffer.
    remote = client.get_object(Bucket=BUCKET, Key=pack_key)["Body"].read()
    sidecar = client.get_object(Bucket=BUCKET, Key=manifest_key)["Body"].read()
    if (len(remote) != len(expected_pack)
            or sha256(remote) != sha256(expected_pack)
            or sidecar != expected_manifest):
        raise Rejected("S3 data not identical to approved payload")
    meta = parse_manifest(sidecar, sha256(expected_manifest))
    if meta["pack_sha256"] != sha256(remote):
        raise Rejected("manifest points to different physical PACK")
    start = meta["seek_table_offset"]
    table = parse_seek_table(remote[start:], len(remote))
    if len(table) != len(editions) or len(meta["entries"]) != len(editions):
        raise Rejected("incorrect chapter count")
    for expected, entry in zip(editions, meta["entries"]):
        chapter, edition, version, previous, raw = expected
        if (entry["edition_id"] != edition or entry["sha256"] != sha256(raw)
                or entry["raw_bytes"] != len(raw)):
            raise Rejected("manifest disagrees with immutable edition")
        offset, size, raw_size = table[entry["frame"]]
        if offset != entry["offset"] or size != entry["encoded_bytes"]:
            raise Rejected("seek table disagrees with catalog")
        if raw_size != len(raw):
            raise Rejected("seek table raw byte length differs")
        verified = extract_range(
            lambda a, n: remote[a:a+n], len(remote),
            sidecar, sha256(sidecar), edition)
        if verified != raw:
            raise Rejected("edition raw bytes mismatch")
    return {"pack_key": pack_key, "manifest_key": manifest_key,
            "pack_sha": sha256(remote), "manifest_sha": sha256(sidecar)}


def upload_and_verify(conn, client, job_id, owner, epoch, fingerprint,
                      editions, crash_after_put=False):
    if fingerprint_of(specification(editions)) != fingerprint:
        raise Rejected("caller supplied different batch")
    with conn.transaction():
        gate(conn, job_id, owner, epoch, fingerprint, ("reserved", "verified"))
    pack, manifest = build_pack([(e[1], e[4]) for e in editions], level=3)
    pack_key = f"{PREFIX}/{job_id}/{sha256(pack)}.zst"
    manifest_key = f"{PREFIX}/{job_id}/{sha256(manifest)}.manifest.json"
    immutable_put(client, pack_key, pack)
    immutable_put(client, manifest_key, manifest)
    if crash_after_put:
        raise SimulatedCrash("after S3 PUT, before verification/PG receipt")
    receipt = actual_pack_receipt(
        client, pack_key, manifest_key, pack, manifest, editions)
    with conn.transaction():
        job = gate(conn, job_id, owner, epoch, fingerprint, ("reserved", "verified"))
        # Repair a verified receipt only with identical physical contents.
        if job["state"] == "verified" and any(
            job[key] != receipt[key] for key in receipt):
            raise Rejected("cannot change verified content receipt")
        conn.execute(
            """UPDATE drop_fault_ci.jobs SET
               pack_key=%s, manifest_key=%s, pack_sha=%s,
               manifest_sha=%s, state='verified' WHERE job_id=%s""",
            (receipt["pack_key"], receipt["manifest_key"],
             receipt["pack_sha"], receipt["manifest_sha"], job_id))
    return receipt


def finalize(conn, job_id, owner, epoch, fingerprint, failure=None):
    """The only transaction that makes a verified batch visible."""
    if failure not in (None, "after_first_row", "before_commit", "after_commit"):
        raise ValueError("unknown crash boundary")
    with conn.transaction():
        row = conn.execute(
            "SELECT * FROM drop_fault_ci.jobs WHERE job_id=%s FOR UPDATE",
            (job_id,)).fetchone()
        if row and row["fingerprint"] == fingerprint and row["state"] == "published":
            return "already-published"
        job = gate(conn, job_id, owner, epoch, fingerprint, ("verified",))
        if not all(job[x] for x in ("pack_key", "manifest_key",
                                   "pack_sha", "manifest_sha")):
            raise Rejected("missing S3 verification receipt")
        items = conn.execute(
            "SELECT * FROM drop_fault_ci.job_items WHERE job_id=%s ORDER BY edition_id",
            (job_id,)).fetchall()
        if not items:
            raise Rejected("empty publication")
        for index, item in enumerate(items):
            conn.execute(
                """INSERT INTO drop_fault_ci.editions
                   (edition_id, chapter_id, version_num, raw_sha,
                    pack_key, manifest_key, manifest_sha)
                   VALUES (%s,%s,%s,%s,%s,%s,%s)""",
                (item["edition_id"], item["chapter_id"], item["version_num"],
                 item["raw_sha"], job["pack_key"], job["manifest_key"],
                 job["manifest_sha"]))
            if item["expected_prev"] is None:
                conn.execute(
                    """INSERT INTO drop_fault_ci.current_chapter
                       (chapter_id, edition_id) VALUES (%s,%s)""",
                    (item["chapter_id"], item["edition_id"]))
            else:
                updated = conn.execute(
                    """UPDATE drop_fault_ci.current_chapter
                       SET edition_id=%s
                       WHERE chapter_id=%s AND edition_id=%s""",
                    (item["edition_id"], item["chapter_id"],
                     item["expected_prev"]))
                if updated.rowcount != 1:
                    raise Rejected("optimistic edition pointer changed")
            if failure == "after_first_row" and index == 0:
                raise SimulatedCrash("during PG batch publication")
        conn.execute("UPDATE drop_fault_ci.jobs SET state='published' WHERE job_id=%s",
                     (job_id,))
        if failure == "before_commit":
            raise SimulatedCrash("before PG COMMIT")
    if failure == "after_commit":
        raise SimulatedCrash("PG COMMIT succeeded, response lost")
    return "published"


def takeover(conn, job_id, new_owner):
    with conn.transaction():
        current = conn.execute(
            "SELECT * FROM drop_fault_ci.jobs WHERE job_id=%s FOR UPDATE",
            (job_id,)).fetchone()
        if current is None or current["state"] not in ("reserved", "verified"):
            raise Rejected("job cannot be reclaimed")
        next_epoch = current["lease_epoch"] + 1
        conn.execute(
            """UPDATE drop_fault_ci.jobs
               SET owner=%s, lease_epoch=%s,
                   lease_until=clock_timestamp() + interval '5 minutes',
                   state='reserved', pack_key=NULL, pack_sha=NULL,
                   manifest_key=NULL, manifest_sha=NULL
               WHERE job_id=%s""",
            (new_owner, next_epoch, job_id))
        return next_epoch


def cancel(conn, job_id):
    with conn.transaction():
        current = conn.execute(
            "SELECT state FROM drop_fault_ci.jobs WHERE job_id=%s FOR UPDATE",
            (job_id,)).fetchone()
        if current is None or current["state"] == "published":
            raise Rejected("cannot cancel already published job")
        conn.execute("UPDATE drop_fault_ci.jobs SET state='cancelled' WHERE job_id=%s",
                     (job_id,))


def count(conn, table, where=None, params=()):
    sql = f"SELECT count(*) AS n FROM drop_fault_ci.{table}"
    if where:
        sql += " WHERE " + where
    return conn.execute(sql, params).fetchone()["n"]


def get_current_id(conn, chapter_id):
    item = conn.execute(
        "SELECT edition_id FROM drop_fault_ci.current_chapter WHERE chapter_id=%s",
        (chapter_id,)).fetchone()
    return None if item is None else item["edition_id"]


def read_authorized(conn, client, chapter_id, reader=None, saved_id=None):
    """An illustrative authority boundary, not TOR's real reader ACL."""
    if saved_id:
        if reader is None:
            raise Rejected("saved edition requires authenticated reader")
        row = conn.execute(
            """SELECT e.* FROM drop_fault_ci.purchased_pins p
               JOIN drop_fault_ci.editions e ON e.edition_id=p.edition_id
               WHERE p.reader_id=%s AND p.edition_id=%s AND e.chapter_id=%s""",
            (reader, saved_id, chapter_id)).fetchone()
        if row is None:
            raise Rejected("reader lacks exact purchased V1")
    else:
        row = conn.execute(
            """SELECT e.* FROM drop_fault_ci.current_chapter c
               JOIN drop_fault_ci.editions e ON e.edition_id=c.edition_id
               WHERE c.chapter_id=%s""",
            (chapter_id,)).fetchone()
        if row is None:
            raise Rejected("unpublished chapter has no reader pointer")
    manifest = client.get_object(
        Bucket=BUCKET, Key=row["manifest_key"])["Body"].read()
    # Trusted hash lives only in the PG edition record.
    parse_manifest(manifest, row["manifest_sha"])
    metadata = json.loads(manifest)
    def exact_range(offset, size):
        response = client.get_object(
            Bucket=BUCKET, Key=row["pack_key"],
            Range=f"bytes={offset}-{offset+size-1}")
        if response["ResponseMetadata"]["HTTPStatusCode"] != 206:
            raise Rejected("S3 returned wrong status for Range GET")
        return response["Body"].read()
    result = extract_range(exact_range, metadata["pack_encoded_bytes"],
                           manifest, row["manifest_sha"], row["edition_id"])
    if sha256(result) != row["raw_sha"]:
        raise Rejected("edition content digest mismatch")
    return result


def audit_orphans(conn, client):
    # Detection only. No storage deletion or GC is permitted by this script.
    current = set()
    for row in conn.execute(
        "SELECT pack_key, manifest_key FROM drop_fault_ci.editions"):
        current.add(row["pack_key"])
        current.add(row["manifest_key"])
    listed, token = set(), None
    while True:
        args = {"Bucket": BUCKET, "Prefix": PREFIX + "/", "MaxKeys": 1000}
        if token:
            args["ContinuationToken"] = token
        page = client.list_objects_v2(**args)
        listed.update(x["Key"] for x in page.get("Contents", []))
        if not page.get("IsTruncated", False):
            break
        token = page["NextContinuationToken"]
    return {"objects": len(listed), "referenced_objects": len(current),
            "orphan_objects": len(listed - current),
            "orphan_sha256": [sha256(key.encode()) for key in sorted(listed - current)]}


def case(label, fn):
    t0 = time.perf_counter()
    fn()
    RESULTS.append({"scenario": label, "status": "PASS",
                    "milliseconds": round((time.perf_counter()-t0)*1000, 2)})


def must_reject(fn, exception=(Rejected, SimulatedCrash)):
    try:
        fn()
    except exception:
        return
    raise AssertionError("the forbidden action was accepted")


def run_experiment(conn, client, url):
    # V1 is a genuine, verified S3 pack and a pinned purchase; the V2 code must
    # leave its logical edition and physical bytes unchanged.
    v1_body = ("Сохранённая купленная V1, неизменяемая.\n" * 25).encode()
    v1 = [("book-a/ch-1", "book-a-v1", 1, None, v1_body)]
    fp1, epoch1 = reserve(conn, "initial-v1", "author", v1)
    case("initial-v1-verifies-before-first-publication", lambda: (
        upload_and_verify(conn, client, "initial-v1", "author", epoch1, fp1, v1),
        finalize(conn, "initial-v1", "author", epoch1, fp1)))
    conn.execute(
        "INSERT INTO drop_fault_ci.purchased_pins VALUES (%s,%s)",
        ("reader-with-purchase", "book-a-v1"))
    assert read_authorized(conn, client, "book-a/ch-1",
                           "reader-with-purchase", "book-a-v1") == v1_body

    v2_body = ("Вторая версия, новая редакция.\n" * 25).encode()
    other_body = ("Отдельная глава из того же атомарного импорта.\n" * 30).encode()
    v2 = [
        ("book-a/ch-1", "book-a-v2", 2, "book-a-v1", v2_body),
        ("book-a/ch-2", "book-a-ch2-v1", 1, None, other_body),
    ]
    fp2, epoch2 = reserve(conn, "batch-v2", "worker-old", v2)

    case("object-store-down-blocks-publication", lambda: (
        must_reject(lambda: upload_and_verify(conn, StorageDown(), "batch-v2",
                    "worker-old", epoch2, fp2, v2), exception=ConnectionError),
        must_reject(lambda: finalize(conn, "batch-v2", "worker-old", epoch2, fp2))))
    assert get_current_id(conn, "book-a/ch-1") == "book-a-v1"

    case("crash-after-put-before-receipt-leaves-private-orphan", lambda: (
        must_reject(lambda: upload_and_verify(conn, client, "batch-v2",
                    "worker-old", epoch2, fp2, v2, crash_after_put=True),
                    exception=SimulatedCrash),
        must_reject(lambda: finalize(conn, "batch-v2", "worker-old", epoch2, fp2))))
    assert count(conn, "editions") == 1
    assert get_current_id(conn, "book-a/ch-2") is None

    case("retry-verifies-real-remote-content", lambda:
         upload_and_verify(conn, client, "batch-v2", "worker-old",
                           epoch2, fp2, v2))
    case("fingerprint-rejects-modified-import", lambda: must_reject(
        lambda: finalize(conn, "batch-v2", "worker-old", epoch2, "not-approved")))
    case("mid-transaction-crash-rolls-back-all-editions", lambda: must_reject(
        lambda: finalize(conn, "batch-v2", "worker-old", epoch2, fp2,
                         failure="after_first_row"), exception=SimulatedCrash))
    assert count(conn, "editions") == 1
    assert get_current_id(conn, "book-a/ch-1") == "book-a-v1"
    assert get_current_id(conn, "book-a/ch-2") is None

    case("before-commit-crash-keeps-old-V1", lambda: must_reject(
        lambda: finalize(conn, "batch-v2", "worker-old", epoch2, fp2,
                         failure="before_commit"), exception=SimulatedCrash))
    assert count(conn, "editions") == 1

    case("commit-then-lost-acknowledgment-idempotent-retry", lambda: (
        must_reject(lambda: finalize(conn, "batch-v2", "worker-old", epoch2, fp2,
                                    failure="after_commit"), exception=SimulatedCrash),
        assert_already(finalize(conn, "batch-v2", "worker-old", epoch2, fp2))))
    assert count(conn, "editions") == 3
    assert get_current_id(conn, "book-a/ch-1") == "book-a-v2"
    assert get_current_id(conn, "book-a/ch-2") == "book-a-ch2-v1"

    case("purchased-v1-survives-v2", lambda: (
        assert_bytes(read_authorized(conn, client, "book-a/ch-1",
                                     "reader-with-purchase", "book-a-v1"), v1_body),
        assert_bytes(read_authorized(conn, client, "book-a/ch-1"), v2_body)))
    case("wrong-reader-cannot-use-purchased-v1", lambda: must_reject(
        lambda: read_authorized(conn, client, "book-a/ch-1",
                                "other-reader", "book-a-v1")))

    v3body = ("Третья версия, противостоит гонке.\n" * 25).encode()
    v3 = [("book-a/ch-1", "book-a-v3", 3, "book-a-v2", v3body)]
    fp3, ep3 = reserve(conn, "concurrent-v3", "new-worker", v3)
    case("concurrent-v3-upload", lambda:
         upload_and_verify(conn, client, "concurrent-v3", "new-worker",
                           ep3, fp3, v3))

    def finalize_from_new_connection():
        with connection(url) as local:
            return finalize(local, "concurrent-v3", "new-worker", ep3, fp3)

    def simultaneous_commit():
        with ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(pool.map(lambda _: finalize_from_new_connection(), (0, 1)))
        if sorted(outcomes) != ["already-published", "published"]:
            raise AssertionError(f"duplicate concurrent finalize outcomes: {outcomes}")
        assert count(conn, "editions") == 4
        assert get_current_id(conn, "book-a/ch-1") == "book-a-v3"
    case("concurrent-finalizers-single-commit", simultaneous_commit)

    stale = [("book-a/ch-3", "stale-v1", 1, None,
              ("Данные для смены воркера.\n" * 15).encode())]
    fpst, oldepoch = reserve(conn, "lease-fence", "stale-owner", stale)
    case("stale-upload-verify-then-takeover", lambda:
         upload_and_verify(conn, client, "lease-fence", "stale-owner",
                           oldepoch, fpst, stale))
    epoch_new = takeover(conn, "lease-fence", "current-owner")
    case("stale-worker-fenced-before-finalize", lambda: must_reject(
        lambda: finalize(conn, "lease-fence", "stale-owner", oldepoch, fpst)))
    case("reclaimed-worker-reverifies-and-publishes", lambda: (
        upload_and_verify(conn, client, "lease-fence", "current-owner",
                          epoch_new, fpst, stale),
        finalize(conn, "lease-fence", "current-owner", epoch_new, fpst)))

    to_cancel = [("book-a/ch-4", "cancelled-v1", 1, None, b"cancel content")]
    cancel_fp, cancel_epoch = reserve(conn, "cancelled", "owner", to_cancel)
    cancel(conn, "cancelled")
    case("cancelled-import-denied", lambda: (
        must_reject(lambda: upload_and_verify(
            conn, client, "cancelled", "owner", cancel_epoch, cancel_fp, to_cancel)),
        must_reject(lambda: finalize(
            conn, "cancelled", "owner", cancel_epoch, cancel_fp))))

    expired = [("book-a/ch-5", "expired-v1", 1, None, b"expired content")]
    fp_exp, ep_exp = reserve(conn, "expired", "owner", expired)
    conn.execute("""UPDATE drop_fault_ci.jobs
                    SET lease_until=clock_timestamp()- interval '10 seconds'
                    WHERE job_id='expired'""")
    case("expired-lease-denies-publication", lambda: must_reject(
        lambda: upload_and_verify(conn, client, "expired",
                                  "owner", ep_exp, fp_exp, expired)))
    assert get_current_id(conn, "book-a/ch-5") is None

    tamper = [("book-a/ch-6", "tampered-v1", 1, None, b"verified original")]
    fp_t, ep_t = reserve(conn, "tamper", "owner", tamper)
    correct_pack, manifest = build_pack([(tamper[0][1], tamper[0][4])])
    key = f"{PREFIX}/tamper/{sha256(correct_pack)}.zst"
    # A wrong remote value claims the correct immutable key.
    client.put_object(Bucket=BUCKET, Key=key,
                      Body=correct_pack[:-1] + bytes([correct_pack[-1] ^ 0x40]))
    case("tampered-s3-object-is-not-accepted-on-retry", lambda: must_reject(
        lambda: upload_and_verify(conn, client, "tamper",
                                  "owner", ep_t, fp_t, tamper)))
    assert get_current_id(conn, "book-a/ch-6") is None
    case("no-text-when-storage-unavailable", lambda: must_reject(
        lambda: read_authorized(conn, StorageDown(), "book-a/ch-1"),
        exception=ConnectionError))

    def read_with_dead_pg():
        with connection(url) as dead:
            dead.close()
            read_authorized(dead, client, "book-a/ch-1")
    case("no-text-when-postgres-unavailable", lambda: must_reject(
        read_with_dead_pg, exception=psycopg.Error))

    report = audit_orphans(conn, client)
    assert report["orphan_objects"] >= 2, "must detect unpublished private orphans"
    assert count(conn, "purchased_pins") == 1
    assert count(conn, "editions") == 5
    assert read_authorized(conn, client, "book-a/ch-1",
                           "reader-with-purchase", "book-a-v1") == v1_body
    return report


def assert_already(value):
    if value != "already-published":
        raise AssertionError(f"idempotent retry returned {value}")


def assert_bytes(a, b):
    if a != b:
        raise AssertionError("immutable V1/V2 source bytes differ")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", default="tor-store-fault-results.json")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        first = [("book/ch", "edition-v1", 1, None, b"chapter")]
        second = [("book/ch", "edition-v1", 1, None, b"chapter")]
        assert fingerprint_of(specification(first)) == fingerprint_of(specification(second))
        assert fingerprint_of(specification(first)) != fingerprint_of(
            specification([("book/ch", "edition-v1", 1, None, b"other")]))
        assert sha256(b"") == hashlib.sha256(b"").hexdigest()
        print("TOR-STORE PG/S3 atomicity fingerprint preflight: PASS")
        return
    url, endpoint = assert_local_environment()
    started = time.perf_counter()
    client = s3_client(endpoint)
    client.head_bucket(Bucket=BUCKET)
    with connection(url) as conn:
        setup_database(conn)
        orphans = run_experiment(conn, client, url)
        all_published = count(conn, "editions")
        all_pins = count(conn, "purchased_pins")
        published_jobs = count(conn, "jobs", "state = 'published'")
        verified_jobs = count(conn, "jobs", "state = 'verified'")
    summary = {
        "kind": "DROP-B05-B06-disposable-PG18-MinIO-fault-injection",
        "head_sha": os.getenv("GITHUB_SHA", "local"),
        "passed": len(RESULTS),
        "failed": 0, "scenarios": RESULTS,
        "published_immutable_editions": all_published,
        "purchased_pins_retained": all_pins,
        "published_jobs": published_jobs,
        "verified_unpublished_jobs": verified_jobs,
        "reconciliation": orphans,
        "elapsed_seconds": round(time.perf_counter()-started, 3),
        "production_schema_or_S3_modified": False,
    }
    Path(args.output).write_text(
        json.dumps(summary, ensure_ascii=False, indent=2)+"\n",
        encoding="utf-8")
    print("# TOR-STORE DROP-B05/B06: real PG18 + disposable MinIO")
    print(f"PASS: {summary['passed']} isolated fault/rights/lease scenarios. "
          f"Published editions={all_published}; purchased pins={all_pins}; "
          f"detected private orphan objects={orphans['orphan_objects']}")
    for item in RESULTS:
        print("- PASS " + item["scenario"])
    if os.getenv("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as out:
            out.write("\n## TOR-STORE DROP-B05/B06: PG18/S3 fault injection\n\n"
                      f"{len(RESULTS)} scenarios PASS, all editions and pins "
                      "verified on disposable services. No production migrations.\n\n"
                      + "\n".join("- PASS " + i["scenario"] for i in RESULTS)
                      + "\n")


if __name__ == "__main__":
    main()
