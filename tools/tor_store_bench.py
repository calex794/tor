#!/usr/bin/env python3
"""TOR-STORE disposable benchmark. No private Google Drive text leaves Drive."""
from __future__ import annotations
import argparse
import bisect
import json
import os
import random
import statistics
import time
import urllib.request
import zlib
from pathlib import Path


def pct(values, p):
    values = sorted(values)
    return round(values[(len(values) - 1) * p // 100], 3)


def corpus(n, seed):
    cfg = json.loads(Path(__file__).with_name("tor_store_corpus_profiles.json").read_text())
    rng = random.Random(seed)
    syllables = ("ла", "ро", "вен", "ти", "не", "ми", "ва", "ста", "ко",
                 "ни", "па", "да", "ле", "ша", "ри", "му", "ли", "се",
                 "то", "гра", "ка", "бо", "че", "на", "све", "во", "ре", "три")
    words = ["".join(rng.choice(syllables) for _ in range(rng.randint(2, 5)))
             for _ in range(6000)]
    cumulative, total = [], 0.0
    for i in range(len(words)):
        total += 1 / ((i + 1) ** 1.08)
        cumulative.append(total)
    profiles = cfg["profiles"]
    weights = [p["chapters"] for p in profiles]
    chapters = []
    for _ in range(n):
        prof = rng.choices(profiles, weights=weights)[0]
        q = rng.random()
        points = ((0.0, prof["p10"] * .78), (.1, prof["p10"]),
                  (.5, prof["p50"]), (.9, prof["p90"]),
                  (1.0, prof["p90"] * 1.35))
        target = prof["p50"]
        for (a, x), (b, y) in zip(points, points[1:]):
            if q <= b:
                target = int(x + (y - x) * (q - a) / (b - a))
                break
        para_bytes = max(70, target // prof["paragraphs_per_chapter"])
        paragraphs, size = [], 0
        while size < target:
            tokens, byte_count = [], 0
            while byte_count < para_bytes:
                w = words[bisect.bisect_left(cumulative, rng.random() * total)]
                tokens.append(w)
                byte_count += len(w.encode()) + 1
            paragraph = " ".join(tokens).capitalize() + rng.choice((".", ".", "!", "?")) + "\n\n"
            paragraphs.append(paragraph)
            size += len(paragraph.encode())
        chapters.append("".join(paragraphs).encode())
    return chapters


def pg_benchmark(blobs, samples):
    import psycopg
    url = os.environ["TOR_BENCH_PG_URL"]
    assert "tor_store_bench" in url and "127.0.0.1" in url
    with psycopg.connect(url) as conn:
        with conn.cursor() as cur:
            cur.execute("CREATE TABLE bench_chapters (id bigint PRIMARY KEY, body text NOT NULL)")
            cur.execute("SELECT pg_current_wal_lsn()::text")
            start_lsn = cur.fetchone()[0]
            t0 = time.perf_counter()
            with cur.copy("COPY bench_chapters (id, body) FROM STDIN") as copy:
                for i, blob in enumerate(blobs, 1):
                    copy.write_row((i, blob.decode()))
            conn.commit()
            write_s = time.perf_counter() - t0
            cur.execute("SELECT pg_total_relation_size('bench_chapters'::regclass), pg_relation_size('bench_chapters'::regclass)")
            total_bytes, heap_bytes = cur.fetchone()
            cur.execute("SELECT pg_wal_lsn_diff(pg_current_wal_lsn(), %s::pg_lsn)::bigint", (start_lsn,))
            wal_bytes = cur.fetchone()[0]
            durations = []
            for i in samples:
                t0 = time.perf_counter()
                cur.execute("SELECT body FROM bench_chapters WHERE id = %s", (i,))
                assert cur.fetchone()[0].encode() == blobs[i - 1]
                durations.append((time.perf_counter() - t0) * 1000)
            return dict(objects=len(blobs), stored_bytes=total_bytes,
                        heap_bytes=heap_bytes, wal_bytes=wal_bytes,
                        write_seconds=round(write_s, 3),
                        read_p50_ms=pct(durations, 50), read_p95_ms=pct(durations, 95))


def s3_benchmark(blobs, samples):
    import boto3
    from botocore import UNSIGNED
    from botocore.config import Config
    from botocore.exceptions import ClientError

    endpoint = os.environ["TOR_BENCH_S3_ENDPOINT"]
    assert endpoint == "http://127.0.0.1:9000"
    client = boto3.client("s3", endpoint_url=endpoint, region_name="us-east-1",
                          aws_access_key_id=os.environ["TOR_BENCH_S3_USER"],
                          aws_secret_access_key=os.environ["TOR_BENCH_S3_PASS"],
                          config=Config(signature_version="s3v4", s3={"addressing_style": "path"}))
    bucket = "tor-storage-disposable-bench"
    client.create_bucket(Bucket=bucket)
    t0 = time.perf_counter()
    for i, blob in enumerate(blobs, 1):
        client.put_object(Bucket=bucket, Key=f"single/{i:08d}", Body=blob)
    put_seconds = time.perf_counter() - t0

    unsigned = boto3.client("s3", endpoint_url=endpoint, region_name="us-east-1",
                            config=Config(signature_version=UNSIGNED, s3={"addressing_style": "path"}))
    anonymous_denied = False
    try:
        unsigned.get_object(Bucket=bucket, Key="single/00000001")
    except ClientError as exc:
        anonymous_denied = exc.response.get("ResponseMetadata", {}).get("HTTPStatusCode") == 403
    assert anonymous_denied, "private bucket is anonymously readable"
    durations = []
    for i in samples:
        t0 = time.perf_counter()
        data = client.get_object(Bucket=bucket, Key=f"single/{i:08d}")["Body"].read()
        assert data == blobs[i - 1]
        durations.append((time.perf_counter() - t0) * 1000)
    report = {
        "single": dict(objects=len(blobs), stored_bytes=sum(map(len, blobs)),
                       write_seconds=round(put_seconds, 3),
                       read_p50_ms=pct(durations, 50), read_p95_ms=pct(durations, 95))
    }
    for group in (20, 100):
        pointers, stored_bytes, objects = {}, 0, 0
        cross_example = None
        t0 = time.perf_counter()
        for start in range(0, len(blobs), group):
            objects += 1
            key = f"packed-{group}/{objects:07d}"
            packed = bytearray()
            first_two = []
            for i in range(start, min(start + group, len(blobs))):
                encoded = zlib.compress(blobs[i], 6)
                offset = len(packed)
                pointers[i + 1] = (key, offset, len(encoded))
                first_two.append((offset, len(encoded)))
                packed.extend(encoded)
            if cross_example is None and len(first_two) >= 2:
                cross_example = (key, first_two[1])
            stored_bytes += len(packed)
            client.put_object(Bucket=bucket, Key=key, Body=bytes(packed))
        write_s = time.perf_counter() - t0
        durations = []
        for i in samples:
            key, offset, size = pointers[i]
            t0 = time.perf_counter()
            part = client.get_object(Bucket=bucket, Key=key,
                                     Range=f"bytes={offset}-{offset + size - 1}")["Body"].read()
            assert zlib.decompress(part) == blobs[i - 1]
            durations.append((time.perf_counter() - t0) * 1000)
        cross_member_accessible = False
        if cross_example is not None:
            key, (offset, size) = cross_example
            signed = client.generate_presigned_url("get_object",
                        Params={"Bucket": bucket, "Key": key}, ExpiresIn=60)
            request = urllib.request.Request(
                signed, headers={"Range": f"bytes={offset}-{offset + size - 1}"})
            with urllib.request.urlopen(request, timeout=10) as response:
                cross_member_accessible = response.status == 206 and len(response.read()) == size
        assert cross_member_accessible, "unexpected presigned Range behavior"
        report[f"pack_{group}"] = dict(objects=objects, stored_bytes=stored_bytes,
            write_seconds=round(write_s, 3),
            read_p50_ms=pct(durations, 50), read_p95_ms=pct(durations, 95),
            presigned_url_can_read_other_member=cross_member_accessible)
    report["security"] = {
        "anonymous_object_denied": anonymous_denied,
        "signed_pack_url_exposes_other_member": report["pack_20"]["presigned_url_can_read_other_member"]
    }
    return report


def cost_projection(report, n):
    out = {}
    for name in ("single", "pack_20", "pack_100"):
        row = report[name]
        scale = 60_000_000 / n
        objects = round(row["objects"] * scale)
        gib = row["stored_bytes"] * scale / (1024 ** 3)
        monthly_put = max(0, objects / 12 - 10000) / 1000 * 0.5692
        out[name] = {"objects": objects, "estimated_gib": round(gib, 1),
                     "storage_rub_per_month": round(max(0, gib - 1) * 2.376, 1),
                     "put_rub_first_year": round(monthly_put * 12, 1)}
    return out


def summary_md(result):
    n = result["sample"]["chapters"]
    lines = ["# TOR-STORE / isolated benchmark", "",
             "**Method:** synthetic Russian prose calibrated against aggregate sizes "
             "of 976 real chapters from four private Drive files. Original texts were "
             "not transferred to GitHub. This is not a measurement on the original texts.", "",
             f"Generated {n:,} synthetic chapters; mean {result['sample']['mean_bytes']:,} B/chapter; "
             f"zlib ratio {result['sample']['zlib_ratio']:.3f}.", "",
             "| Back end | Objects/rows | Stored MiB | Write sec | Read p50 / p95 ms |",
             "|---|---:|---:|---:|---:|"]
    for name, row in [("PostgreSQL 18", result["postgresql"])] + [
        (f"Local MinIO {name}", result["minio"][name]) for name in ("single", "pack_20", "pack_100")]:
        lines.append(f"| {name} | {row['objects']:,} | {row['stored_bytes']/2**20:.1f} "
                     f"| {row['write_seconds']:.2f} | {row['read_p50_ms']:.2f} / {row['read_p95_ms']:.2f} |")
    lines += ["", f"PostgreSQL WAL bytes: **{result['postgresql']['wal_bytes']:,}**.", "",
        "### Security",
        f"- Anonymous access denied: **{result['minio']['security']['anonymous_object_denied']}**.",
        "- **Packed S3 presigned URL exposes neighboring ranges**. Plain direct "
        "signed grants must NEVER be used to mix paid/hidden and public chapters.", "",
        "### Cost projection, 60 million chapters (Yandex STANDARD, modeled)",
        "| Layout | Objects | Stored GiB | Storage RUB/month | PUT RUB/year 1 |",
        "|---|---:|---:|---:|---:|"]
    for name, row in result["forecast"].items():
        lines.append(f"| {name} | {row['objects']:,} | {row['estimated_gib']:.1f} | "
                     f"{row['storage_rub_per_month']:.0f} | {row['put_rub_first_year']:.0f} |")
    lines += ["", "Caveats: local MinIO is **not** Yandex S3, CI runner bandwidth is not "
              "production, corpus is synthetic, no real TOR authorization or backup/restore "
              "costs measured, PUT pricing assumes equal imports each month.", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--chapters", type=int, default=2400)
    parser.add_argument("--seed", type=int, default=34916)
    parser.add_argument("--output", default="tor-store-results.json")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    assert 30 <= args.chapters <= 10000, "bounded test only"
    t0 = time.perf_counter()
    blobs = corpus(args.chapters, args.seed)
    sizes = list(map(len, blobs))
    result = {
        "source": "non-reconstructable Drive-derived chapter-size statistics; generated proxy",
        "sample": {"chapters": args.chapters, "input_mib": round(sum(sizes) / 2**20, 2),
                   "mean_bytes": round(statistics.mean(sizes)),
                   "p50_bytes": pct(sizes, 50), "p95_bytes": pct(sizes, 95),
                   "zlib_ratio": round(sum(len(zlib.compress(x, 6)) for x in blobs) / sum(sizes), 4)},
        "generation_seconds": round(time.perf_counter() - t0, 2)
    }
    if not args.dry_run:
        rng = random.Random(args.seed + 1)
        sample = rng.sample(range(1, len(blobs) + 1), min(150, len(blobs)))
        result["postgresql"] = pg_benchmark(blobs, sample)
        result["minio"] = s3_benchmark(blobs, sample)
        result["forecast"] = cost_projection(result["minio"], len(blobs))
    Path(args.output).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    if args.dry_run:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        md = summary_md(result)
        print(md)
        if os.getenv("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
                f.write(md)


if __name__ == "__main__":
    main()
