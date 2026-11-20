#!/usr/bin/env python3
"""TOR-STORE disposable benchmark. No private Google Drive text leaves Drive."""
from __future__ import annotations
import argparse
import bisect
import json
import math
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
                        read_p50_ms=pct(durations, 50), read_p95_ms=pct(durations, 95),
                        read_p99_ms=pct(durations, 99))


def packed_groups(parts, max_items=None, target_bytes=None):
    """Yield nonempty groups of chapter indices without splitting encoded frames."""
    assert (max_items is None) != (target_bytes is None)
    pending, used = [], 0
    for index, part in enumerate(parts):
        if pending and ((max_items is not None and len(pending) >= max_items)
                        or (target_bytes is not None and used + len(part) > target_bytes)):
            yield pending
            pending, used = [], 0
        pending.append(index)
        used += len(part)
    if pending:
        yield pending


def self_test():
    """Small bounded preflight of grouping and independently decodable frames."""
    import zstandard as zstd
    source = [b"a" * 400, b"b" * 500, b"c" * 600, b"d" * 700]
    z = zstd.ZstdCompressor(level=3)
    encoded = [z.compress(x) for x in source]
    for kwargs in ({"max_items": 2}, {"target_bytes": len(encoded[0]) + len(encoded[1])}):
        groups = list(packed_groups(encoded, **kwargs))
        assert [i for group in groups for i in group] == list(range(len(source)))
        for group in groups:
            cursor = 0
            container = b"".join(encoded[i] for i in group)
            for i in group:
                part = container[cursor:cursor + len(encoded[i])]
                assert zstd.ZstdDecompressor().decompress(part) == source[i]
                cursor += len(encoded[i])
            assert cursor == len(container)
    assert list(packed_groups([], max_items=2)) == []
    sample = {"single_zstd3": {"objects": 3, "stored_bytes": 3 * 400},
              "pack_4mib_zstd3": {"objects": 1, "stored_bytes": 3 * 400},
              "pack_100_zstd3": {"objects": 1, "stored_bytes": 3 * 400}}
    projected = cost_projection(sample, measured_chapters=3, annual_chapters=3_000_000)
    assert projected["single_zstd3"]["objects"] == 3_000_000
    assert projected["pack_100_zstd3"]["objects"] == 30_000
    assert projected["pack_4mib_zstd3"]["objects"] == math.ceil(
        (3 * 400 * 1_000_000) / (4 * 2**20))
    print("TOR-STORE codec/grouping/cost preflight: PASS")


def s3_benchmark(blobs, samples):
    """Single vs pack on identical compressed frames. Local MinIO, not provider latency."""
    import boto3
    import zstandard as zstd
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

    zstd_c = zstd.ZstdCompressor(level=3)
    zstd_d = zstd.ZstdDecompressor()
    encoders = {"raw": lambda b: b,
                "zlib6": lambda b: zlib.compress(b, 6),
                "zstd3": zstd_c.compress}
    decoders = {"raw": lambda b: b,
                "zlib6": zlib.decompress,
                "zstd3": zstd_d.decompress}

    # Precompress once: both layouts use exactly the same bytes for each chapter.
    # Report codec CPU time separately, rather than hiding it inside PACK writes.
    compressed = {}
    compression = {}
    input_bytes = sum(map(len, blobs))
    for codec, encode in encoders.items():
        t0 = time.perf_counter()
        parts = [encode(b) for b in blobs]
        seconds = time.perf_counter() - t0
        compressed[codec] = parts
        compression[codec] = {"stored_bytes": sum(map(len, parts)),
                              "ratio": round(sum(map(len, parts)) / input_bytes, 4),
                              "encode_seconds": round(seconds, 3)}
    report = {"compression": compression}

    unsigned = boto3.client("s3", endpoint_url=endpoint, region_name="us-east-1",
                            config=Config(signature_version=UNSIGNED, s3={"addressing_style": "path"}))
    anonymous_denied = False

    def timings(values):
        return {"read_p50_ms": pct(values, 50), "read_p95_ms": pct(values, 95),
                "read_p99_ms": pct(values, 99)}

    def verified_read(name, codec, pointers, expected_objects, write_seconds, stored_bytes):
        parts = compressed[codec]
        durations = []
        for i in samples:
            key, offset, size = pointers[i]
            kwargs = {"Bucket": bucket, "Key": key}
            if offset is not None:
                kwargs["Range"] = f"bytes={offset}-{offset + size - 1}"
            t0 = time.perf_counter()
            response = client.get_object(**kwargs)
            data = response["Body"].read()
            assert len(data) == size and data == parts[i - 1], f"{name}: corrupt encoded member {i}"
            if offset is not None:
                assert response["ResponseMetadata"]["HTTPStatusCode"] == 206
                assert response["ContentRange"].startswith(f"bytes {offset}-{offset + size - 1}/")
            assert decoders[codec](data) == blobs[i - 1], f"{name}: corrupt chapter {i}"
            durations.append((time.perf_counter() - t0) * 1000)
        return dict(objects=expected_objects, stored_bytes=stored_bytes,
                    write_seconds=round(write_seconds, 3), codec=codec,
                    **timings(durations))

    for codec, parts in compressed.items():
        name = f"single_{codec}"
        pointers = {}
        t0 = time.perf_counter()
        for i, blob in enumerate(parts, 1):
            key = f"{name}/{i:08d}"
            pointers[i] = (key, None, len(blob))
            client.put_object(Bucket=bucket, Key=key, Body=blob)
        write_seconds = time.perf_counter() - t0
        report[name] = verified_read(name, codec, pointers, len(parts),
                                     write_seconds, sum(map(len, parts)))

    try:
        unsigned.get_object(Bucket=bucket, Key="single_raw/00000001")
    except ClientError as exc:
        anonymous_denied = exc.response.get("ResponseMetadata", {}).get("HTTPStatusCode") == 403
    assert anonymous_denied, "private bucket is anonymously readable"

    layouts = [
        ("pack_20_zlib6", "zlib6", {"max_items": 20}),
        ("pack_100_zlib6", "zlib6", {"max_items": 100}),
        ("pack_20_zstd3", "zstd3", {"max_items": 20}),
        ("pack_100_zstd3", "zstd3", {"max_items": 100}),
    ] + [(f"pack_{mib}mib_zstd3", "zstd3", {"target_bytes": mib * 2**20})
         for mib in (1, 4, 8, 16, 32)]

    cross_example = None
    for name, codec, grouping in layouts:
        parts = compressed[codec]
        pointers, stored_bytes, objects = {}, 0, 0
        t0 = time.perf_counter()
        for group in packed_groups(parts, **grouping):
            objects += 1
            key = f"{name}/{objects:07d}"
            packed = bytearray()
            for i in group:
                offset = len(packed)
                encoded = parts[i]
                pointers[i + 1] = (key, offset, len(encoded))
                packed.extend(encoded)
            if name == "pack_20_zstd3" and cross_example is None and len(group) >= 2:
                neighbor_id = group[1] + 1
                cross_example = (key, pointers[neighbor_id], parts[neighbor_id - 1])
            stored_bytes += len(packed)
            client.put_object(Bucket=bucket, Key=key, Body=bytes(packed))
        write_seconds = time.perf_counter() - t0
        assert len(pointers) == len(blobs)
        assert stored_bytes == sum(map(len, parts)), "pack payload bytes differ from Single"
        report[name] = verified_read(name, codec, pointers, objects, write_seconds, stored_bytes)

    cross_member_accessible = False
    if cross_example is not None:
        key, (_, offset, size), expected = cross_example
        signed = client.generate_presigned_url("get_object",
                    Params={"Bucket": bucket, "Key": key}, ExpiresIn=60)
        request = urllib.request.Request(
            signed, headers={"Range": f"bytes={offset}-{offset + size - 1}"})
        with urllib.request.urlopen(request, timeout=10) as response:
            cross_member_accessible = (response.status == 206 and response.read() == expected)
    assert cross_member_accessible, "unexpected presigned Range behavior"

    # Exact standard Seekable Zstd 0.1.0 + immutable JSON sidecar. The
    # compressed frames are verified byte-identical to the Single Zstd3 layout,
    # so this row measures the true physical seek-table/manifest overhead.
    from tor_store_seekable import build_pack, extract_range, extract_cached_range, sha256
    seek_name = "pack_100_seekable_zstd3"
    bundles, edition_locations = [], {}
    table_bytes = manifest_stored_bytes = 0
    t0 = time.perf_counter()
    for ordinal, group in enumerate(packed_groups(compressed["zstd3"], max_items=100), 1):
        edition_data = [(f"edition-{i + 1:08d}", blobs[i]) for i in group]
        pack_bytes, manifest_bytes = build_pack(edition_data, level=3)
        metadata = json.loads(manifest_bytes)
        assert metadata["frames"] == len(group)
        for entry, i in zip(metadata["entries"], group):
            a, b = entry["offset"], entry["encoded_bytes"]
            assert pack_bytes[a:a + b] == compressed["zstd3"][i]
        key = f"{seek_name}/{ordinal:07d}"
        bundles.append((key, pack_bytes, manifest_bytes))
        table_bytes += metadata["seek_table_bytes"]
        manifest_stored_bytes += len(manifest_bytes)
        for i in group:
            edition_locations[i + 1] = (key, len(pack_bytes))
    build_seconds = time.perf_counter() - t0

    t0 = time.perf_counter()
    for key, pack_bytes, manifest_bytes in bundles:
        client.put_object(Bucket=bucket, Key=key, Body=pack_bytes)
        client.put_object(Bucket=bucket, Key=key + ".manifest.json", Body=manifest_bytes)
    seek_write_seconds = time.perf_counter() - t0

    # A full verification GET at upload time is deliberately a separate
    # measurement. Later range reads trust the catalog's checked manifest SHA.
    t0 = time.perf_counter()
    checked_bundles = {}
    for key, pack_bytes, manifest_bytes in bundles:
        remote_pack = client.get_object(Bucket=bucket, Key=key)["Body"].read()
        remote_manifest = client.get_object(
            Bucket=bucket, Key=key + ".manifest.json")["Body"].read()
        assert sha256(remote_pack) == sha256(pack_bytes)
        assert sha256(remote_manifest) == sha256(manifest_bytes)
        checked_bundles[key] = (remote_manifest, sha256(manifest_bytes))
    seek_verify_seconds = time.perf_counter() - t0

    seek_durations, cached_durations = [], []
    for sample_number, i in enumerate(samples):
        key, total_size = edition_locations[i]
        remote_manifest, trusted_digest = checked_bundles[key]

        def s3_range(offset, size):
            response = client.get_object(Bucket=bucket, Key=key,
                                         Range=f"bytes={offset}-{offset + size - 1}")
            assert response["ResponseMetadata"]["HTTPStatusCode"] == 206
            assert response["ContentRange"].startswith(f"bytes {offset}-{offset + size - 1}/")
            return response["Body"].read()

        # Alternate cold/warm call order to limit systematic HTTP-cache bias.
        modes = ("cold", "cached") if sample_number % 2 == 0 else ("cached", "cold")
        for mode in modes:
            t0 = time.perf_counter()
            if mode == "cold":
                recovered = extract_range(s3_range, total_size, remote_manifest,
                                          trusted_digest, f"edition-{i:08d}")
                seek_durations.append((time.perf_counter() - t0) * 1000)
            else:
                recovered = extract_cached_range(s3_range, total_size, remote_manifest,
                                                 trusted_digest, f"edition-{i:08d}")
                cached_durations.append((time.perf_counter() - t0) * 1000)
            assert recovered == blobs[i - 1]
    cached_stats = timings(cached_durations)
    report[seek_name] = {
        "objects": len(bundles) * 2,  # One pack + one versioned manifest each
        "stored_bytes": sum(len(p) + len(m) for _, p, m in bundles),
        "write_seconds": round(seek_write_seconds, 3),
        "build_seconds": round(build_seconds, 3),
        "verify_full_get_seconds": round(seek_verify_seconds, 3),
        "seek_table_bytes": table_bytes,
        "manifest_bytes": manifest_stored_bytes,
        "read_range_requests": 2,
        "cached_read_range_requests": 1,
        "cached_read_p50_ms": cached_stats["read_p50_ms"],
        "cached_read_p95_ms": cached_stats["read_p95_ms"],
        "cached_read_p99_ms": cached_stats["read_p99_ms"],
        "codec": "zstd3", **timings(seek_durations),
    }

    report["security"] = {
        "anonymous_object_denied": anonymous_denied,
        "signed_pack_url_exposes_other_member": cross_member_accessible,
    }
    # The simple PACK rows have in-memory offsets and no seek table/manifest.
    # The pack_100_seekable_zstd3 row uses persisted standard Seekable Zstd plus
    # a verified immutable manifest sidecar; it is still a CI prototype, not a TOR ACL.
    return report


def cost_projection(report, measured_chapters, annual_chapters):
    """Tariff-model-only projection at full yearly volume; not a provider invoice."""
    scale = annual_chapters / measured_chapters
    out = {}
    for name, row in report.items():
        if name in ("security", "compression"):
            continue
        projected_payload_bytes = row["stored_bytes"] * scale
        if name.startswith("single_"):
            objects = annual_chapters
        elif name.split("_")[1].endswith("mib"):
            # A small sample's last partially-filled pack must not be replicated
            # thousands of times. Calculate only the IDEAL lower bound on objects;
            # production placement and max-size fragmentation may require more.
            target_mib = int(name.split("_")[1].removesuffix("mib"))
            objects = math.ceil(projected_payload_bytes / (target_mib * 2**20))
        else:
            per_pack = int(name.split("_")[1])
            objects = math.ceil(annual_chapters / per_pack)
            if "_seekable_" in name:
                # A separate immutable manifest is a second object + PUT.
                objects *= 2
        gib = projected_payload_bytes / (1024 ** 3)
        # Assumes uniform imports across 12 months and historical modeled STANDARD
        # prices; excludes GET, egress, manifest/index, replicas and backups.
        monthly_put = max(0, objects / 12 - 10_000) / 1000 * 0.5692
        out[name] = {"objects": objects, "estimated_gib": round(gib, 1),
                     "storage_rub_per_month_at_full_size": round(max(0, gib - 1) * 2.376, 1),
                     "put_rub_first_year": round(monthly_put * 12, 1)}
    return out


def summary_md(result):
    n = result["sample"]["chapters"]
    lines = ["# TOR-STORE / fair compression + layout experiment", "",
             "**Data:** synthetic Russian-like text with chapter sizes calibrated against "
             "976 private Drive chapters; no original text/Drive URL uploaded.", "",
             f"Generated {n:,} chapters, {result['sample']['input_mib']:.2f} MiB raw.", "",
             "Compression was performed **once per codec before any S3 write**. "
             "Single and PACK use byte-identical compressed frames. "
             "S3 write timings exclude compression; PG uses bulk COPY rather than "
             "sequential PUT. Values are local-runner measurements, not Yandex S3.", "",
             "| Codec | Encoded MiB | Ratio | Encode seconds |",
             "|---|---:|---:|---:|"]
    for codec, item in result["minio"]["compression"].items():
        lines.append(f"| {codec} | {item['stored_bytes']/2**20:.2f} "
                     f"| {item['ratio']:.3f} | {item['encode_seconds']:.2f} |")
    lines += ["", "| Layout | Objects | Payload MiB | S3 write sec | Verified read p50/p95/p99 ms |",
              "|---|---:|---:|---:|---:|"]
    for name, row in [("PostgreSQL 18", result["postgresql"])] + [
        (f"MinIO {name}", item) for name, item in result["minio"].items()
        if name not in ("security", "compression")]:
        lines.append(f"| {name} | {row['objects']:,} | {row['stored_bytes']/2**20:.2f} "
                     f"| {row['write_seconds']:.2f} | {row['read_p50_ms']:.2f} / "
                     f"{row['read_p95_ms']:.2f} / {row.get('read_p99_ms', 0):.2f} |")
    seek = result["minio"]["pack_100_seekable_zstd3"]
    lines += ["", "### Standard Seekable read modes (same physical pack/manifest)", "",
              "| Path | S3 Range GETs/read | p50 / p95 / p99 ms |",
              "|---|---:|---:|",
              f"| Cold (seek table + frame) | {seek['read_range_requests']} "
              f"| {seek['read_p50_ms']:.2f} / {seek['read_p95_ms']:.2f} / "
              f"{seek['read_p99_ms']:.2f} |",
              f"| Trusted verified index (frame only) | {seek['cached_read_range_requests']} "
              f"| {seek['cached_read_p50_ms']:.2f} / {seek['cached_read_p95_ms']:.2f} / "
              f"{seek['cached_read_p99_ms']:.2f} |", "",
              "The second path requires a trusted manifest checksum, immutable pack "
              "identity and full verification at ingest. It does not replace TOR "
              "authorization; each frame still has a source SHA-256 check. "
              "Call order alternates to reduce read-cache bias.", ""]
    lines += ["", f"PostgreSQL WAL bytes: **{result['postgresql']['wal_bytes']:,}**.", "",
              "**Important:** each PACK stores independent compressed frames with an "
              "in-memory offset index. PACK payload bytes equal the respective "
              "compressed Single payload bytes. The separate **pack_100_seekable_zstd3** "
              "row measures a real standard seek-table and JSON manifest sidecar, "
              "including extra bytes and PUT; it makes two verified Range GETs "
              "per sampled chapter. Neither layout is a production TOR ACL.", "",
              "### Security", 
              f"- Anonymous read denied: **{result['minio']['security']['anonymous_object_denied']}**.",
              f"- Presigned PACK URL exposes other Range: "
              f"**{result['minio']['security']['signed_pack_url_exposes_other_member']}**.", "",
              "### Illustrative Yandex STANDARD model, not actual billing", ""]
    for scenario, forecast in (("3.65 million chapters/year", result["forecast_3650k"]),
                               ("60 million chapters/year", result["forecast_60m"])):
        lines += [f"#### {scenario}", "",
                  "| Layout | Objects | GiB at year end | RUB/month at year-end size | PUT RUB/year 1 |",
                  "|---|---:|---:|---:|---:|"]
        for name in ("single_raw", "single_zlib6", "single_zstd3",
                     "pack_100_zlib6", "pack_100_zstd3", "pack_100_seekable_zstd3",
                     "pack_4mib_zstd3"):
            row = forecast[name]
            lines.append(f"| {name} | {row['objects']:,} | {row['estimated_gib']:.1f} "
                         f"| {row['storage_rub_per_month_at_full_size']:.0f} "
                         f"| {row['put_rub_first_year']:.0f} |")
        lines.append("")
    lines += ["**Cost model warning:** size-bounded PACK object counts use an IDEAL " 
              "lower bound of ceil(projected compressed payload / target size), "
              "not measured production packing. Fixed-count groups use ceil(chapters / group); "
              "seekable groups also count one immutable manifest object per pack. "
              "The storage RUB/month column is the charge at YEAR-END occupancy, not an "
              "average across the first year.", "",
              "**Limitations:** no real cloud/network charges, backups, or PG index/WAL "
              "retention; simple PACK rows exclude manifest/index overhead, but the "
              "Seekable row includes its own table and sidecar; synthetic text is not representative "
              "for real compression ratios; fixed read order may warm local caches; "
              "pg COPY vs sequential S3 PUT is not ingestion-throughput parity; "
              "modeled tariff constants must be revalidated at decision time.", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--chapters", type=int, default=2400)
    parser.add_argument("--seed", type=int, default=34916)
    parser.add_argument("--output", default="tor-store-results.json")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    assert 30 <= args.chapters <= 10000, "bounded benchmark: 30..10000 chapters"
    t0 = time.perf_counter()
    blobs = corpus(args.chapters, args.seed)
    sizes = list(map(len, blobs))
    result = {
        "source": "non-reconstructable Drive-derived sizes; synthetic proxy only",
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
        result["forecast_3650k"] = cost_projection(result["minio"], len(blobs), 3_650_000)
        result["forecast_60m"] = cost_projection(result["minio"], len(blobs), 60_000_000)
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
