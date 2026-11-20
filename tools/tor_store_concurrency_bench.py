#!/usr/bin/env python3
"""TOR-STORE DROP-B03: comparable concurrent Single/PACK writes to loopback MinIO.

Disposable, synthetic, isolated. No production traffic/credentials/Drive text.
The codec runs before timed writes for all layouts; do NOT compare these times
with PostgreSQL COPY, Yandex API latencies or a real TOR publication transaction.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import hashlib
import json
import os
from pathlib import Path
import random
import resource
import statistics
import time

from tor_store_bench import corpus, pct
from tor_store_seekable import build_pack
import zstandard as zstd

THREAD_COUNTS = (1, 8, 32)
GROUP_SIZE = 100
DEFAULT_CHAPTERS = 4000
DEFAULT_REPEATS = 2
BUCKET = "tor-storage-disposable-bench"
LAYOUTS = ("single_zstd3", "pack_100_zstd3", "pack_100_seekable_zstd3")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def prepare_payloads(blobs):
    """Keep one immutable set of encoded frames across every test/layout."""
    require(len(blobs) >= 3200, "need 3200+ chapters to populate 32 pack workers")
    t0 = time.perf_counter()
    cctx = zstd.ZstdCompressor(level=3)
    frames = [cctx.compress(b) for b in blobs]
    codec_seconds = time.perf_counter() - t0
    output = {"single_zstd3": [b for b in frames]}
    metadata = {"single_zstd3": {"objects": len(frames),
                                   "stored_bytes": sum(map(len, frames))}}
    t0 = time.perf_counter()
    pack = []
    for start in range(0, len(frames), GROUP_SIZE):
        pack.append(b"".join(frames[start:start + GROUP_SIZE]))
    fixed_prep_s = time.perf_counter() - t0
    output["pack_100_zstd3"] = pack
    metadata["pack_100_zstd3"] = {
        "objects": len(pack), "stored_bytes": sum(map(len, pack))}
    assert metadata["pack_100_zstd3"]["stored_bytes"] == metadata["single_zstd3"]["stored_bytes"]

    t0 = time.perf_counter()
    seekable = []
    for start in range(0, len(frames), GROUP_SIZE):
        raw = blobs[start:start + GROUP_SIZE]
        packed, manifest = build_pack(
            [(f"immutable-edition-{start+i+1:08d}", b)
             for i, b in enumerate(raw)], level=3)
        manifest_model = json.loads(manifest)
        for item, expected in zip(manifest_model["entries"], frames[start:start + GROUP_SIZE]):
            off = item["offset"]
            end = off + item["encoded_bytes"]
            assert packed[off:end] == expected, "Seekable frame differs from Single"
        seekable.extend((packed, manifest))
    seekable_prep_s = time.perf_counter() - t0
    output["pack_100_seekable_zstd3"] = seekable
    metadata["pack_100_seekable_zstd3"] = {
        "objects": len(seekable),
        "stored_bytes": sum(map(len, seekable)),
        "packs": len(seekable) // 2,
        "manifest_objects": len(seekable) // 2,
    }
    return output, metadata, {
        "zstd3_once_seconds": round(codec_seconds, 4),
        "simple_pack_prepare_seconds": round(fixed_prep_s, 4),
        "seekable_pack_and_manifest_prepare_seconds": round(seekable_prep_s, 4),
    }


def put_batch(client, layout, tasks, workers, repeat, seed, chapter_count):
    """Time only S3 PUT operations with exactly the requested worker bound."""
    require(workers in THREAD_COUNTS and 0 <= repeat < 4, "invalid benchmark stage")
    require(tasks, "empty upload set")
    order = list(range(len(tasks)))
    random.Random(seed).shuffle(order)
    names = [(f"drop-b03/{layout}/w{workers}/r{repeat}/{index:07d}",
              tasks[index]) for index in order]

    def put_one(key, body):
        t0 = time.perf_counter()
        response = client.put_object(Bucket=BUCKET, Key=key, Body=body)
        elapsed_ms = (time.perf_counter() - t0) * 1000
        if response["ResponseMetadata"]["HTTPStatusCode"] != 200:
            raise RuntimeError("MinIO rejected PUT")
        return elapsed_ms

    durations = []
    cpu0 = time.process_time()
    t0 = time.perf_counter()
    # boto3 clients can be used across threads; the caller configures pool_size
    # >= max_workers so a connection-pool warning does not distort measurements.
    with ThreadPoolExecutor(max_workers=workers) as executor:
        future_to_key = {executor.submit(put_one, key, body): key
                         for key, body in names}
        for future in as_completed(future_to_key):
            durations.append(future.result())
    wall_s = time.perf_counter() - t0
    cpu_s = time.process_time() - cpu0
    assert len(durations) == len(tasks)

    # A verified publication cannot rely on a random sample. Read back EVERY
    # object after timed PUT, compare full bytes and SHA-256, and measure this
    # stage separately at the SAME concurrency. Seekable includes all manifests.
    def verify_one(key, expected):
        t0 = time.perf_counter()
        response = client.get_object(Bucket=BUCKET, Key=key)
        if response["ResponseMetadata"]["HTTPStatusCode"] != 200:
            raise RuntimeError("MinIO rejected verification GET")
        remote = response["Body"].read()
        if len(remote) != len(expected) or sha256(remote) != sha256(expected):
            raise AssertionError(f"full-object readback mismatch for {layout}")
        return (time.perf_counter() - t0) * 1000

    verify_durations = []
    cpu1 = time.process_time()
    t1 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = [executor.submit(verify_one, key, body) for key, body in names]
        for future in as_completed(futures):
            verify_durations.append(future.result())
    verify_s = time.perf_counter() - t1
    verify_cpu_s = time.process_time() - cpu1
    assert len(verify_durations) == len(names)
    stored_bytes = sum(map(len, tasks))
    total_wall_s = wall_s + verify_s
    return {
        "layout": layout, "workers": workers, "repeat": repeat,
        "chapters": chapter_count, "objects": len(tasks), "stored_bytes": stored_bytes,
        "write_wall_seconds": round(wall_s, 4),
        "write_cpu_seconds": round(cpu_s, 4),
        "put_per_second": round(len(tasks) / wall_s, 2),
        "mib_per_second": round(stored_bytes / 2**20 / wall_s, 2),
        "put_latency_p50_ms": pct(durations, 50),
        "put_latency_p95_ms": pct(durations, 95),
        "put_latency_p99_ms": pct(durations, 99),
        "verify_full_get_wall_seconds": round(verify_s, 4),
        "verify_cpu_seconds": round(verify_cpu_s, 4),
        "verify_full_gets": len(verify_durations),
        "verify_get_p95_ms": pct(verify_durations, 95),
        "write_plus_verify_seconds": round(total_wall_s, 4),
        "verified_chapters_per_second": round(chapter_count / total_wall_s, 2),
    }


def markdown_report(result):
    out = [
        "# TOR-STORE: comparable Single vs PACK concurrent PUT",
        "",
        "**Experimental MinIO on localhost**, not Yandex S3 and not a TOR publication transaction.",
        "All layouts write the **same independent Zstd-3 compressed frames**. "
        "Seekable has a standards-compatible seek table and one immutable JSON "
        "manifest per pack. Compression and PACK preparation are timed separately.",
        f"Sample: {result['chapters']:,} synthetic chapters; "
        f"{result['input_raw_mib']:.2f} MiB raw; {result['zstd3_mib']:.2f} MiB compressed. "
        f"Each layout has {result['repeats']} trials.",
        "",
        "| Layout | Workers | S3 objects | Median PUT wall s | "
        "Median full-GET verify s | Median PUT+verify s | "
        "Chapters/s incl. verify | PUT p95 ms |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for layout in LAYOUTS:
        for workers in THREAD_COUNTS:
            rows = [row for row in result["runs"]
                    if row["layout"] == layout and row["workers"] == workers]
            assert len(rows) == result["repeats"]
            write_s = statistics.median(r["write_wall_seconds"] for r in rows)
            verify_s = statistics.median(r["verify_full_get_wall_seconds"] for r in rows)
            total_s = statistics.median(r["write_plus_verify_seconds"] for r in rows)
            throughput = statistics.median(r["verified_chapters_per_second"] for r in rows)
            p95 = statistics.median(r["put_latency_p95_ms"] for r in rows)
            out.append(
                f"| {layout} | {workers} | {rows[0]['objects']:,} | {write_s:.3f} "
                f"| {verify_s:.3f} | {total_s:.3f} | {throughput:.1f} | {p95:.2f} |")
    out += [
        "", "### Preprocessing is NOT inside PUT wall clock",
        "",
        "| Stage | Seconds |",
        "|---|---:|",
    ]
    for k, v in result["preparation"].items():
        out.append(f"| {k} | {v:.4f} |")
    out += [
        "", f"Peak process max RSS: {result['process_peak_rss_kib']:,} KiB "
        "(process-wide observed, includes corpus and all layouts).",
        "",
        "### Interpretation limits",
        "",
        "- Equal concurrent workers does not imply equal per-chapter latency: "
        "one pack PUT publishes many chapters only after manifest, object verification "
        "and PG transaction, none of which are measured in this PUT wall time.",
        "- PACK100 has ~chapters/100 tasks; with 4k chapters, 40 PACK "
        "PUTs limit useful worker parallelism. Seekable adds one manifest PUT/pack.",
        "- Timed PUT excludes Zstd encoding and PACK building. A separate "
        "fully-timed verification GET checks ALL stored physical objects "
        "against prepared SHA-256 at equal concurrency. The total PUT+GET "
        "still excludes a TOR PostgreSQL finalization transaction, "
        "rights, retries, external WAN latency and cache/multi-region.",
        "- All task orders are seeded and shuffled, but loopback OS page caches "
        "and background runner activity can bias two repeats. "
        "Do not use ratios as production forecasts.",
        "- Full readback validates each physical object; actual TOR edition "
        "authorization and consistent placement/purchase mapping were NOT tested.",
        "- No real Yandex credential/bucket/VPS was used. "
        "S3 PUT semantics and prices must be measured separately in a disposable "
        "vendor bucket, with explicit budget and authorization.",
        "",
    ]
    return "\n".join(out)


def self_test():
    source = [b"small book chapter " * 10 for _ in range(3200)]
    payloads, meta, prep = prepare_payloads(source)
    assert meta["single_zstd3"]["objects"] == 3200
    assert meta["pack_100_zstd3"]["objects"] == 32
    assert meta["pack_100_seekable_zstd3"]["objects"] == 64
    assert meta["pack_100_zstd3"]["stored_bytes"] == meta["single_zstd3"]["stored_bytes"]
    assert meta["pack_100_seekable_zstd3"]["stored_bytes"] > meta["single_zstd3"]["stored_bytes"]
    assert all(key in prep for key in
               ("zstd3_once_seconds", "simple_pack_prepare_seconds",
                "seekable_pack_and_manifest_prepare_seconds"))
    print("TOR-STORE concurrency layout preflight: PASS")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--chapters", type=int, default=DEFAULT_CHAPTERS)
    parser.add_argument("--seed", type=int, default=34916)
    parser.add_argument("--repeats", type=int, default=DEFAULT_REPEATS)
    parser.add_argument("--output", default="tor-store-concurrency-results.json")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    require(3200 <= args.chapters <= 10_000, "concurrency chapters must be 3200..10000")
    require(1 <= args.repeats <= 3, "repeats must be 1..3")
    import boto3
    from botocore.config import Config
    endpoint = os.environ["TOR_BENCH_S3_ENDPOINT"]
    require(endpoint == "http://127.0.0.1:9000", "benchmark requires loopback MinIO")
    client = boto3.client(
        "s3", endpoint_url=endpoint, region_name="us-east-1",
        aws_access_key_id=os.environ["TOR_BENCH_S3_USER"],
        aws_secret_access_key=os.environ["TOR_BENCH_S3_PASS"],
        config=Config(signature_version="s3v4",
                      max_pool_connections=64, connect_timeout=3, read_timeout=20,
                      retries={"total_max_attempts": 1},
                      s3={"addressing_style": "path"}),
    )
    client.head_bucket(Bucket=BUCKET)  # bucket created by preceding benchmark step
    originals = corpus(args.chapters, args.seed)
    payloads, meta, prep = prepare_payloads(originals)

    stages = [(layout, workers) for layout in LAYOUTS for workers in THREAD_COUNTS]
    results = []
    for repeat in range(args.repeats):
        stage_order = stages[:]
        random.Random(args.seed + 555 + repeat).shuffle(stage_order)
        for layout, workers in stage_order:
            result = put_batch(
                client, layout, payloads[layout], workers, repeat,
                seed=args.seed ^ (repeat * 65537 + workers),
                chapter_count=args.chapters)
            results.append(result)
    report = {
        "profile": "DROP-B03 synthetic loopback",
        "head_sha": os.environ.get("GITHUB_SHA", "local"),
        "chapters": args.chapters,
        "repeats": args.repeats,
        "input_raw_mib": round(sum(map(len, originals)) / 2**20, 4),
        "zstd3_mib": round(meta["single_zstd3"]["stored_bytes"] / 2**20, 4),
        "preparation": prep,
        "payload_layouts": meta,
        "thread_counts": list(THREAD_COUNTS),
        "runs": results,
        "process_peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
    }
    Path(args.output).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    markdown = markdown_report(report)
    print(markdown)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("\n" + markdown + "\n")


if __name__ == "__main__":
    main()
