#!/usr/bin/env python3
"""Stable required check that verifies the selected TOR CI suite jobs."""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable

from ci_test_map import SUITE_JOB_IDS


class GateError(ValueError):
    """Missing, malformed, failed, or skipped required CI evidence."""


FOUNDATION_MARKERS = {
    "paid-private-e2e": {
        "four source formats": r"FND03_SUPPORTED_UPLOADS_OK formats=4",
        "ten books with exact inventory": (
            r"FND07_MULTI_BOOK_IMPORT_OK books=10 chapters=30 versions=30 "
            r"objects=30 missing=0 unexpected=0"
        ),
        "one hundred books": r"FND03_CONCURRENT_CATALOG_OK books=100 editions=100 objects=100 workers=2",
        "one hundred thousand chapters": r"DROP04D_LOAD_PASS chapters=100000 batches=[1-9][0-9]* seconds=[0-9]+\.[0-9]+",
        "worker restart": r"DROP04D_WORKER_RETRY_PASS: reclaimed stage, exact PACK, no SINGLE, atomic job",
        "read-only reconciliation": (
            r"DROP07D_PG_MINIO_OPERATOR_PASS: bounded LIST\+PG classification across two pages, "
            r"fenced checkpoint, unknown never GC"
        ),
    },
    "sealed-remote-copy": {
        "encrypted backup": r"I11_PRIVATE_S3_ENCRYPTED_BACKUP_VERIFIED",
        "restored PACK": r"DROP07_RESTORED_PACK_HISTORICAL_V1_OK",
        "restored saved and Layer access": r"FND05_RESTORED_FREE_ARCHIVE_HTTP_OK",
        "saved FREE and revoked denial": r"DEPLOY07C_SAVED_FREE_LAYER_AND_REVOKED_DENIAL_OK",
        "corrupt restored PACK denial": r"DEPLOY07C_CORRUPT_RESTORED_PACK_DENIED",
        "WAL recovery": r"I11_PITR_ISOLATED_DRILL_OK report=\S+",
    },
    "purchased-v1-recovery": {
        "purchased V1 and KEK": r"S334_PURCHASED_V1_AFTER_V2_PG_S3_KEK_RESTORE_PASS",
        "exact V1/V2 database facts": r"DEPLOY07C_PAID_V1_V2_DB_EXACT_OK",
        "missing/wrong keys and outage": r"DEPLOY07C_MISSING_WRONG_KEYS_AND_PROVIDER_DENIED",
        "cross-purchase and anonymous denial": r"DEPLOY07C_CROSS_PURCHASE_AND_ANONYMOUS_DENIED",
        "both restored refunds denied": r"DEPLOY07C_BOTH_RESTORED_REFUNDS_DENIED",
        "source unchanged": r"DEPLOY07C_PAID_SOURCE_UNCHANGED_OK",
        "both historical KEK decryptions": r"S334_HISTORICAL_KEK_V1_V2_RESTORE_PASS",
    },
}


def foundation_receipt(job_id: str, sha: str) -> str:
    if job_id not in FOUNDATION_MARKERS or re.fullmatch(r"[a-f0-9]{40}", sha) is None:
        raise GateError("invalid foundation job or checkout SHA")
    return f"{sha}:{job_id}"


def verify_foundation_log(job_id: str, log: Iterable[str], sha: str) -> str:
    receipt = foundation_receipt(job_id, sha)
    pending = {case: re.compile(pattern) for case, pattern in FOUNDATION_MARKERS[job_id].items()}
    for line in log:
        line = line.rstrip("\r\n")
        for case, pattern in tuple(pending.items()):
            if pattern.fullmatch(line):
                del pending[case]
        if not pending:
            return receipt
    raise GateError(f"{job_id}: missing foundation evidence for {', '.join(pending)}")


def verify(event_name: str, needs: dict, sha: str) -> dict:
    if event_name not in {"pull_request", "push", "workflow_dispatch", "schedule"}:
        raise GateError("unsupported workflow event")
    required_dependencies = {"select", "preflight", *SUITE_JOB_IDS.values()}
    if not isinstance(needs, dict) or set(needs) != required_dependencies:
        raise GateError("unexpected or missing dependency jobs")
    # A successful selector is not sufficient: all expensive jobs depend on
    # this cheap structural/formatting prerequisite, and a skipped preflight
    # must never count as accepted, even when no feature suites were selected.
    preflight = needs["preflight"]
    if not isinstance(preflight, dict) or preflight.get("result") != "success":
        raise GateError("preflight must complete successfully")
    selector = needs["select"]
    if not isinstance(selector, dict):
        raise GateError("invalid selector job evidence")
    result = selector.get("result")
    if result not in {"success", "failure", "skipped", "cancelled"}:
        raise GateError("missing or malformed selector status")
    if result == "cancelled":
        raise GateError("cancelled run must never satisfy required CI")
    if event_name != "pull_request" and result != "success":
        raise GateError("main and manual events require a successful selector")

    fallback_full = result != "success"
    if event_name != "pull_request" or fallback_full:
        selected = dict.fromkeys(SUITE_JOB_IDS, True)
    else:
        output = selector.get("outputs")
        raw = output.get("suites") if isinstance(output, dict) else None
        if not isinstance(raw, str) or not raw:
            raise GateError("successful selector has no suite outputs")
        try:
            selected = json.loads(raw)
        except (TypeError, ValueError) as exc:
            raise GateError("unparseable selector outputs") from exc
        if (not isinstance(selected, dict) or set(selected) != set(SUITE_JOB_IDS)
                or any(type(flag) is not bool for flag in selected.values())):
            raise GateError("selector must return one boolean for every known suite")

    errors = []
    verified = 0
    foundation_jobs = []
    for suite, job_id in SUITE_JOB_IDS.items():
        job = needs[job_id]
        if not isinstance(job, dict):
            errors.append(f"{job_id}: invalid job evidence")
            continue
        state = job.get("result")
        if selected[suite]:
            if state != "success":
                errors.append(f"{job_id}: required {suite} is {state!r}")
            else:
                verified += 1
        elif state == "success":
            verified += 1
        elif state != "skipped":
            errors.append(f"{job_id}: unselected suite is {state!r}")
        if state == "success" and job_id in FOUNDATION_MARKERS:
            outputs = job.get("outputs")
            receipt = outputs.get("foundation_evidence") if isinstance(outputs, dict) else None
            if receipt != foundation_receipt(job_id, sha):
                errors.append(f"{job_id}: missing or mismatched foundation evidence for checkout SHA")
            else:
                foundation_jobs.append(job_id)
    if errors:
        raise GateError("; ".join(errors))
    return {"event": event_name, "selected": sum(selected.values()),
            "verified": verified, "total": len(SUITE_JOB_IDS),
            "fallback_full": fallback_full, "foundation_jobs": sorted(foundation_jobs)}


def main() -> int:
    try:
        sha = os.environ["GITHUB_SHA"]
        if len(sys.argv) == 4 and sys.argv[1] == "record-foundation":
            _, _, job_id, path = sys.argv
            if subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip() != sha:
                raise GateError("foundation test checkout does not match workflow SHA")
            with Path(path).open(encoding="utf-8", errors="replace") as log:
                receipt = verify_foundation_log(job_id, log, sha)
            with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
                output.write(f"foundation_evidence={receipt}\n")
            print(f"TOR_FND07_EVIDENCE_OK job={job_id} cases={len(FOUNDATION_MARKERS[job_id])}")
            return 0
        if len(sys.argv) != 1:
            raise GateError("expected record-foundation JOB LOG or no arguments")
        report = verify(os.environ["GITHUB_EVENT_NAME"], json.loads(os.environ["TOR_CI_NEEDS_JSON"]), sha)
    except (OSError, subprocess.SubprocessError, KeyError, TypeError, ValueError) as exc:
        print(f"TOR_CI_REQUIRED_BLOCKED: {exc}", file=sys.stderr)
        return 1
    print("TOR_CI_REQUIRED_OK " + json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
