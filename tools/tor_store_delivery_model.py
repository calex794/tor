#!/usr/bin/env python3
"""DROP-03 disposable read/cache/cost sensitivity study on a SYNTHETIC chapter corpus.

No secrets, private text, live billing credentials, network calls, or production API.
Prices are snapshot assumptions from official Yandex Object Storage documentation,
not actual charges. Results MUST NOT be presented as real readership/cache statistics.
"""
from __future__ import annotations

import argparse
import bisect
from collections import OrderedDict
import json
import math
from pathlib import Path
import random

import zstandard as zstd

from tor_store_bench import corpus

PRICE_SOURCE = "https://yandex.cloud/ru/docs/storage/pricing"
PRICE_SNAPSHOT = "2026-09-16"
CURRENCY = "RUB, VAT included"
GIB = 1024 ** 3

# Official Russia-region documentation. All quotas reset monthly and are
# service/account shared, not a free allocation for each packed object/bucket.
CLASSES = {
    "STANDARD": dict(storage_rub_gib_month=2.376, put_rub_per_1000=0.5692,
                     get_rub_per_10000=0.46, free_storage_gib=1,
                     free_put_month=10_000, free_get_month=100_000),
    "COLD": dict(storage_rub_gib_month=1.2672, put_rub_per_1000=1.39,
                 get_rub_per_10000=1.15, free_storage_gib=0,
                 free_put_month=0, free_get_month=0),
    "ICE": dict(storage_rub_gib_month=0.6336, put_rub_per_1000=2.79,
                get_rub_per_10000=2.30, free_storage_gib=0,
                free_put_month=0, free_get_month=0),
}

# Outgoing Object Storage -> Internet. Only this tariff band is modeled:
# first 100 GiB/month free, then 1.67994 RUB/GiB until 1 TiB total volume.
# Crossing 1 TiB must be repriced against the full provider price list.
FREE_EGRESS_GIB = 100
FIRST_BAND_EGRESS_RUB_GIB = 1.67994
MODEL_MAX_EGRESS_GIB = 1024
READS_PER_DAY = (10_000, 100_000, 1_000_000)
READ_HIT_SENSITIVITY = (0.0, 0.2, 0.8)
YEARLY_CHAPTERS = (3_650_000, 60_000_000)
BENCH_READS = 50_000


def monthly_s3_cost(*, store_class, storage_gib, puts, gets, egress_gib):
    if store_class not in CLASSES:
        raise ValueError("unknown storage class")
    if any(x < 0 or not math.isfinite(x)
           for x in (storage_gib, puts, gets, egress_gib)):
        raise ValueError("invalid negative or nonfinite quantity")
    rate = CLASSES[store_class]
    stored = (max(0, storage_gib - rate["free_storage_gib"])
              * rate["storage_rub_gib_month"])
    written = max(0, puts - rate["free_put_month"]) / 1000 * rate["put_rub_per_1000"]
    read = max(0, gets - rate["free_get_month"]) / 10_000 * rate["get_rub_per_10000"]
    traffic = None
    if egress_gib <= MODEL_MAX_EGRESS_GIB:
        traffic = max(0, egress_gib - FREE_EGRESS_GIB) * FIRST_BAND_EGRESS_RUB_GIB
    return {
        "storage_rub": round(stored, 3),
        "put_rub": round(written, 3),
        "get_rub": round(read, 3),
        "s3_egress_rub": None if traffic is None else round(traffic, 3),
        "total_known_rub": None if traffic is None else round(stored + written + read + traffic, 3),
        "egress_pricing_complete": traffic is not None,
    }


def s3_delivery_case(*, profile, yearly_chapters, reads_per_day, cache_hit, range_gets,
                     store_class="STANDARD", payload_multiplier=1):
    """At steady year-end storage capacity; not an averaged first-year invoice."""
    if not 0 <= cache_hit <= 1 or range_gets not in (1, 2) or payload_multiplier <= 0:
        raise ValueError("invalid delivery scenario")
    chapters_in_test = profile["sample"]["chapters"]
    if chapters_in_test < 30:
        raise ValueError("insufficient baseline sample")
    layout = profile["minio"]["pack_100_seekable_zstd3"]
    single = profile["minio"]["single_zstd3"]
    if layout["objects"] % 2 or layout["read_range_requests"] != 2:
        raise ValueError("invalid seekable baseline")
    scale = yearly_chapters / chapters_in_test
    # Pack has one manifest sidecar PUT for each pack (100 chapters); half-emptied
    # final packs/manifest minimum sizes can make this an optimistic estimate.
    pack_object_count = 2 * math.ceil(yearly_chapters / 100)
    single_object_count = yearly_chapters
    pack_size_gib = layout["stored_bytes"] * scale / GIB
    single_size_gib = single["stored_bytes"] * scale / GIB
    mean_compressed_bytes = (profile["minio"]["compression"]["zstd3"]["stored_bytes"]
                             / chapters_in_test)
    mean_table_bytes = layout["seek_table_bytes"] / (layout["objects"] // 2)
    # The sidecar is assumed to be in a trusted local index on the hot path.
    # Additional manifest HEAD/GET and checksum validation occur at ingest/refresh,
    # NOT on every read; such refresh costs are NOT included in this model.
    misses = reads_per_day * 30 * (1.0 - cache_hit)
    billed_gets = misses * range_gets
    compressed_bytes = misses * (mean_compressed_bytes * payload_multiplier
                                + (mean_table_bytes if range_gets == 2 else 0))
    egress_gib = compressed_bytes / GIB
    put_month = pack_object_count / 12
    single_put_month = single_object_count / 12

    measured = monthly_s3_cost(
        store_class=store_class, storage_gib=pack_size_gib,
        puts=put_month, gets=billed_gets, egress_gib=egress_gib)
    single_result = monthly_s3_cost(
        store_class=store_class, storage_gib=single_size_gib,
        puts=single_put_month, gets=misses, egress_gib=misses *
        mean_compressed_bytes * payload_multiplier / GIB)
    return {
        "annual_ingest_chapters": yearly_chapters,
        "reads_per_day": reads_per_day,
        "cache_hit_assumption": cache_hit,
        "read_range_gets_per_miss": range_gets,
        "payload_multiplier_assumption": payload_multiplier,
        "store_class": store_class,
        "read_misses_per_month": round(misses),
        "origin_get_per_month": round(billed_gets),
        "origin_egress_gib_per_month": round(egress_gib, 4),
        "origin_total_raw_gib_per_month_at_full_reader_volume": round(
            (reads_per_day * 30 * profile["sample"]["mean_bytes"] / GIB), 4),
        "pack_usable_gib_at_year_end": round(pack_size_gib, 3),
        "single_usable_gib_at_year_end": round(single_size_gib, 3),
        "pack_physical_objects_at_year_end": pack_object_count,
        "pack": measured,
        "single_zstd3": single_result,
        "additional_monthly_delivery_and_ops_break_even_rub": (
            None if single_result["total_known_rub"] is None or
                    measured["total_known_rub"] is None else round(
                single_result["total_known_rub"] - measured["total_known_rub"], 2)),
    }


def cache_replay(encoded_sizes, request_ids, cache_capacity_bytes):
    """Size-bounded LRU storing compressed immutable edition bytes, never ACLs."""
    if cache_capacity_bytes < 0 or not encoded_sizes or any(n <= 0 for n in encoded_sizes):
        raise ValueError("invalid cache inputs")
    items = OrderedDict()
    used = 0
    hits, miss_bytes, misses = 0, 0, 0
    for index in request_ids:
        if index < 0 or index >= len(encoded_sizes):
            raise ValueError("invalid immutable edition index")
        if index in items:
            hits += 1
            items.move_to_end(index)
            continue
        misses += 1
        size = encoded_sizes[index]
        miss_bytes += size
        if cache_capacity_bytes <= 0 or size > cache_capacity_bytes:
            continue
        while used + size > cache_capacity_bytes:
            _, evicted = items.popitem(last=False)
            used -= evicted
        items[index] = size
        used += size
    total = hits + misses
    return {
        "requests": total,
        "hits": hits, "misses": misses, "origin_bytes": miss_bytes,
        "hit_ratio": round(hits / total, 4) if total else 0,
        "cache_capacity_bytes": cache_capacity_bytes,
        "cache_final_bytes": used,
    }


def generate_requests(n, requests, shape, seed):
    """Three hypothetical access traces; NOT real TOR analytics."""
    if n < 30 or requests < 1 or shape not in ("uniform", "zipf", "sequential"):
        raise ValueError("invalid request profile")
    rng = random.Random(seed)
    if shape == "uniform":
        return [rng.randrange(n) for _ in range(requests)]
    cumulative = []
    total = 0.
    for rank in range(n):
        total += 1 / ((rank + 1) ** 1.12)
        cumulative.append(total)

    def popular():
        return bisect.bisect_left(cumulative, rng.random() * total)

    if shape == "zipf":
        return [popular() for _ in range(requests)]
    # Entirely SYNTHETIC book grouping: every 60 adjacent IDs are one book,
    # and 65% of accesses advance to the next chapter of the same book.
    # This is NOT a fitted probability of TOR reading patterns.
    output = []
    current = popular()
    for _ in range(requests):
        if output and rng.random() < .65:
            current = min((current // 60) * 60 + 59, current + 1)
            current = min(n - 1, current)
        else:
            current = popular()
        output.append(current)
    return output


def replay_matrix(sizes, seed):
    total_bytes = sum(sizes)
    capacities = [("none", 0), ("one_percent", total_bytes // 100),
                  ("five_percent", total_bytes // 20),
                  ("twenty_percent", total_bytes // 5)]
    out = []
    for shape in ("uniform", "zipf", "sequential"):
        trace = generate_requests(len(sizes), BENCH_READS, shape, seed + 1)
        for label, cap in capacities:
            r = cache_replay(sizes, trace, cap)
            out.append({"trace": shape, "cache": label, "capacity_bytes": cap,
                        "hit_ratio": r["hit_ratio"],
                        "origin_gib_in_replay": round(r["origin_bytes"] / GIB, 5),
                        "requests": BENCH_READS})
    return out


def self_test():
    sizes = [2, 3, 2]
    assert cache_replay(sizes, [0, 1, 0, 2, 0], 4)["hits"] == 1
    assert cache_replay(sizes, [0, 1, 0, 2, 0], 0)["hits"] == 0
    assert cache_replay(sizes, [0, 1, 0, 2, 0], 7)["hits"] == 2
    assert generate_requests(100, 100, "uniform", 14) == generate_requests(
        100, 100, "uniform", 14)
    assert len(generate_requests(100, 10, "sequential", 8)) == 10
    zero = monthly_s3_cost(
        store_class="STANDARD", storage_gib=1,
        puts=10_000, gets=100_000, egress_gib=100)
    assert zero["total_known_rub"] == 0
    assert monthly_s3_cost(
        store_class="STANDARD", storage_gib=1,
        puts=10_000, gets=110_000, egress_gib=100)["get_rub"] == 0.46
    assert monthly_s3_cost(
        store_class="STANDARD", storage_gib=1,
        puts=10_000, gets=100_000, egress_gib=101)["s3_egress_rub"] == 1.68
    assert monthly_s3_cost(
        store_class="STANDARD", storage_gib=1,
        puts=10_000, gets=100_000, egress_gib=1025)["egress_pricing_complete"] is False
    fixture = {
        "sample": {"chapters": 100, "mean_bytes": 20_000},
        "minio": {
            "pack_100_seekable_zstd3": {
                "objects": 2, "stored_bytes": 500_000,
                "seek_table_bytes": 817, "read_range_requests": 2},
            "single_zstd3": {"stored_bytes": 450_000},
            "compression": {"zstd3": {"stored_bytes": 450_000}},
        },
    }
    a = s3_delivery_case(profile=fixture, yearly_chapters=60_000_000,
                         reads_per_day=100_000, cache_hit=0, range_gets=2)
    b = s3_delivery_case(profile=fixture, yearly_chapters=60_000_000,
                         reads_per_day=100_000, cache_hit=.8, range_gets=1)
    assert a["origin_get_per_month"] == 6_000_000
    assert b["origin_get_per_month"] == 600_000
    assert b["pack"]["get_rub"] < a["pack"]["get_rub"]
    assert b["pack_physical_objects_at_year_end"] == 1_200_000
    print("TOR-STORE delivery tariff/cache/GET sensitivity preflight: PASS")


def markdown_report(data):
    l = ["# TOR-STORE DROP-03: reading, cache, and S3 cost sensitivity", "",
         "**This is a model, NOT measured user traffic, S3 cloud billing, or live latency.**",
         f"Prices: [Yandex Object Storage]({PRICE_SOURCE}), RUB incl. VAT, "
         f"documentation snapshot {PRICE_SNAPSHOT}; validate again before procurement.",
         "Free STANDARD: 1 GiB, 10k PUT and 100k GET monthly. Origin internet egress "
         "first 100 GiB monthly free, 1.67994 RUB/GiB to 1 TiB. "
         "ICE has a 12-month minimum retention charge.", "",
         "### Synthetic workload replay (50k requests on the same 2400 proxy chapters)", "",
         "| Imagined readership | Cache capacity of this synthetic corpus | Hit rate |",
         "|---|---:|---:|"]
    for x in data["replay"]:
        l.append(f"| {x['trace']} | {x['cache']} | {x['hit_ratio']*100:.1f}% |")
    l += ["", "These hit rates are **not portable** to a 60M-edition library. "
          "No measured TOR reader popularity, editorial age, or real book sequence "
          "was supplied. Do NOT assume 80% cache hits in production.", "",
          "### STANDARD monthly delivery sensitivity at year-end archive size",
          "Includes storage and ingest PUT, plus modeled GET and Object Storage egress. "
          "Excludes additional VPS proxy bandwidth, CPU, SSL, Redis/PG, backup, "
          "manifest refresh, integrity audits and operator time.", "",
          "| Annual ingest | Reads/day | Cache assumption | Range/miss | "
          "S3 GET/mo | Origin GiB/mo | PACK S3 RUB/mo | Direct Single S3 RUB/mo |",
          "|---:|---:|---:|---:|---:|---:|---:|---:|"]
    rows = data["scenarios"]
    for x in rows:
        if (x["store_class"] != "STANDARD" or x["payload_multiplier_assumption"] != 1
                or x["cache_hit_assumption"] not in (0., .8)):
            continue
        l.append(f"| {x['annual_ingest_chapters']:,} | {x['reads_per_day']:,} "
                 f"| {x['cache_hit_assumption']*100:.0f}% | {x['read_range_gets_per_miss']} "
                 f"| {x['origin_get_per_month']:,} | {x['origin_egress_gib_per_month']:.2f} "
                 f"| {x['pack']['total_known_rub']:.1f} "
                 f"| {x['single_zstd3']['total_known_rub']:.1f} |")
    l += ["", "### Input assumptions and warnings", "",
          f"- Average Zstd3 encoded frame: {data['mean_zstd3_frame_bytes']:.1f} B. "
          f"Average raw edition: {data['mean_raw_frame_bytes']:.1f} B. "
          "Both from synthetic benchmark, not real book text.",
          "- The direct-Single comparison assumes it already uses the same codec "
          "and shares the same cache hit assumptions. It cannot automatically "
          "serve every restricted edition without TOR authorization.",
          "- A PACK read with a trusted indexed placement uses 1 S3 Range GET. "
          "A cold seek-table lookup uses 2; a manifest GET per read or shared "
          "authorization would add work not in this baseline.",
          "- Prices reflect a hypothetical full archive at year-end; first-year "
          "storage accumulates gradually. Standard free tiers apply across the "
          "service, not separately for each object, book or bucket.",
          "- COLD and ICE retrieval/retention costs are sensitivity checks; "
          "ICE has a 12-month minimum even if items are deleted sooner.",
          "- For payload multiplier >1 we change delivery transfer bytes but "
          "NOT storage, because actual compression of real books is unknown. "
          "Such rows are network sensitivity only, not a coherent revised corpus.",
          "- For origin transfer above 1 TiB/month the tiered price is NOT "
          "modeled and the total is null (fail closed).",
          "- Any local/VPS to end-user traffic, private gateway compute, "
          "concurrent reads, retry rate and actual S3 tail latency require "
          "separate tests.", ""]
    return "\n".join(l)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", help="Existing numeric synthetic benchmark JSON")
    parser.add_argument("--output", default="tor-store-delivery-results.json")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.input:
        parser.error("--input is required except for --self-test")
    source = json.loads(Path(args.input).read_text("utf-8"))
    n = source["sample"]["chapters"]
    if not 30 <= n <= 10_000 or "pack_100_seekable_zstd3" not in source["minio"]:
        raise ValueError("require complete seekable benchmark profile")
    # Deterministically recreate the same synthetic corpus without writing it to
    # output/artifacts. This does NOT inspect/download Google Drive documents.
    blobs = corpus(n, source["sample"].get("seed", 34916))
    compressor = zstd.ZstdCompressor(level=3)
    sizes = [len(compressor.compress(x)) for x in blobs]
    expected = source["minio"]["compression"]["zstd3"]["stored_bytes"]
    if sum(sizes) != expected:
        raise ValueError("synthetic corpus sizes disagree with benchmark profile")
    scenarios = []
    for volume in YEARLY_CHAPTERS:
        for daily in READS_PER_DAY:
            for hit in READ_HIT_SENSITIVITY:
                for mode in (1, 2):
                    for tier in CLASSES:
                        scenarios.append(s3_delivery_case(
                            profile=source, yearly_chapters=volume,
                            reads_per_day=daily, cache_hit=hit,
                            range_gets=mode, store_class=tier))
    data = {
        "kind": "disposable-synthetic-sensitivity",
        "price_source": PRICE_SOURCE,
        "price_snapshot": PRICE_SNAPSHOT,
        "currency": CURRENCY,
        "not_real_traffic": True,
        "sample_chapters": n,
        "mean_raw_frame_bytes": round(source["sample"]["mean_bytes"], 2),
        "mean_zstd3_frame_bytes": round(expected / n, 2),
        "replay": replay_matrix(sizes, 3491626),
        "scenarios": scenarios,
    }
    Path(args.output).write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
    summary = markdown_report(data)
    print(summary)
    import os
    if os.getenv("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write("\n" + summary + "\n")


if __name__ == "__main__":
    main()
