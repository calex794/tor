#!/usr/bin/env python3
"""Offline, fail-closed schema and clock gate for independent TOR DR evidence.

A passing report is REVIEW_READY only, never automatic DR acceptance.
The operator must separately authenticate source evidence and sign off.
No network, secrets, PostgreSQL or S3 are accessed.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import stat
import sys

MAX_REPORT_BYTES = 64 * 1024
RPO_LIMIT_SECONDS = 15 * 60
RTO_LIMIT_SECONDS = 4 * 60 * 60
HEX_SHA = re.compile(r"[0-9a-f]{64}\Z")
HEX_COMMIT = re.compile(r"[0-9a-f]{40}\Z")
UTC_SECONDS = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z\Z")
SAFE_REFERENCE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:/-]{2,127}\Z")

REQUIRED_TRUE = (
    "restore_host_separate",
    "production_untouched",
    "database_pitr_t1_present_t2_absent",
    "wal_post_base_backup_replay_verified",
    "controlled_wal_witness_verified",
    "external_s3_and_iam_separation_verified",
    "object_lock_and_retention_verified",
    "no_live_s3_fallback_verified",
    "full_s3_inventory_verified",
    "historical_free_receipts_verified",
    "private_v1_hash_and_offline_kek_verified",
    "purchased_v1_after_v2_verified",
    "saved_free_v1_after_v2_verified",
    "layer_free_v1_after_deletion_verified",
    "refund_and_unauthorized_acl_denials_verified",
    "layer_revocation_denial_verified",
    "negative_controls_failed_safely",
    "external_alert_delivery_verified",
)
REQUIRED_REFS = (
    "backup_set_ref_redacted",
    "pg_wal_evidence_ref",
    "object_inventory_evidence_ref",
    "kek_recovery_evidence_ref",
    "reader_acl_evidence_ref",
    "negative_controls_evidence_ref",
    "external_alert_evidence_ref",
    "operator_signoff_ref",
)
CLOCK_FIELDS = (
    "t_loss_utc",
    "t_latest_source_commit_utc",
    "t_latest_restored_commit_utc",
    "t_recovered_service_utc",
)
KEYS = set(REQUIRED_TRUE + REQUIRED_REFS + CLOCK_FIELDS) | {
    "schema_version", "status", "source_sha", "postgres_version",
    "pgbackrest_version", "source_inventory_manifest_sha256",
    "measured_rpo_seconds", "measured_rto_seconds",
}


class Refused(ValueError):
    """Safe, non-sensitive DR evidence refusal."""


def unique_object_fields(pairs):
    obj = {}
    for key, value in pairs:
        if key in obj:
            raise Refused("duplicate evidence field")
        obj[key] = value
    return obj


def parse_utc(value):
    if not isinstance(value, str) or not UTC_SECONDS.fullmatch(value):
        raise Refused("UTC timestamps must be exact whole seconds with Z suffix")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise Refused("invalid UTC timestamp") from error


def review_report(report):
    if not isinstance(report, dict) or set(report) != KEYS:
        raise Refused("incomplete or unexpected DR evidence fields")
    if type(report["schema_version"]) is not int or report["schema_version"] != 1:
        raise Refused("unsupported DR evidence schema")
    if report["status"] != "REVIEW_READY":
        raise Refused("report must be REVIEW_READY, not a self-declared acceptance")
    if not isinstance(report["source_sha"], str) or not HEX_COMMIT.fullmatch(report["source_sha"]):
        raise Refused("exact 40-character source commit required")
    if (not isinstance(report["postgres_version"], str)
            or not re.fullmatch(r"18(?:\.\d+)?", report["postgres_version"])):
        raise Refused("PostgreSQL 18 version required")
    if (not isinstance(report["pgbackrest_version"], str)
            or not re.fullmatch(r"\d+(?:\.\d+){1,3}", report["pgbackrest_version"])):
        raise Refused("pgBackRest version required")
    sha = report["source_inventory_manifest_sha256"]
    if not isinstance(sha, str) or not HEX_SHA.fullmatch(sha) or sha == "0" * 64:
        raise Refused("independent pre-incident manifest digest required")
    for flag in REQUIRED_TRUE:
        if report[flag] is not True:
            raise Refused("required independent DR control is missing or false")
    for field in REQUIRED_REFS:
        value = report[field]
        if (not isinstance(value, str) or not SAFE_REFERENCE.fullmatch(value)
                or "://" in value or value.startswith(("http:", "https:"))):
            raise Refused("redacted operator evidence reference missing or unsafe")
    clocks = {key: parse_utc(report[key]) for key in CLOCK_FIELDS}
    lost = clocks["t_loss_utc"]
    source = clocks["t_latest_source_commit_utc"]
    restored = clocks["t_latest_restored_commit_utc"]
    ready = clocks["t_recovered_service_utc"]
    if not restored <= source <= lost <= ready:
        raise Refused("DR evidence has inconsistent witness or recovery clock order")
    rpo = int((source - restored).total_seconds())
    rto = int((ready - lost).total_seconds())
    for field, actual, limit in (
        ("measured_rpo_seconds", rpo, RPO_LIMIT_SECONDS),
        ("measured_rto_seconds", rto, RTO_LIMIT_SECONDS),
    ):
        recorded = report[field]
        if type(recorded) is not int or recorded != actual or recorded > limit:
            raise Refused("DR RPO/RTO missing, inconsistent or outside operator targets")
    return rpo, rto


def load_report(path):
    if path.is_symlink() or not path.is_file():
        raise Refused("regular local report file required")
    if not stat.S_ISREG(path.lstat().st_mode) or path.stat().st_size > MAX_REPORT_BYTES:
        raise Refused("invalid or oversized DR report")
    try:
        raw = path.read_bytes()
        if len(raw) > MAX_REPORT_BYTES:
            raise Refused("oversized DR report")
        obj = json.loads(raw, object_pairs_hook=unique_object_fields)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise Refused("invalid or unreadable DR report") from error
    return obj


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    try:
        rpo, rto = review_report(load_report(args.report))
    except (Refused, OSError, OverflowError) as error:
        detail = str(error) if isinstance(error, Refused) else "unreadable DR report"
        print(f"TOR_I11_DR_EVIDENCE_REFUSED: {detail}", file=sys.stderr)
        return 2
    print(
        "TOR_I11_DR_EVIDENCE_REVIEW_READY "
        f"rpo_seconds={rpo} rto_seconds={rto} "
        "independent_operator_verification_required=true"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
