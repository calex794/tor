#!/usr/bin/env python3
"""Stable required check that verifies the selected TOR CI suite jobs."""
from __future__ import annotations

import json
import os
import sys

from ci_test_map import SUITE_JOB_IDS


class GateError(ValueError):
    """Missing, malformed, failed, or skipped required CI evidence."""


def verify(event_name: str, needs: dict) -> dict:
    if event_name not in {"pull_request", "push", "workflow_dispatch"}:
        raise GateError("unsupported workflow event")
    required_dependencies = {"select", *SUITE_JOB_IDS.values()}
    if not isinstance(needs, dict) or set(needs) != required_dependencies:
        raise GateError("unexpected or missing dependency jobs")
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
    if errors:
        raise GateError("; ".join(errors))
    return {"event": event_name, "selected": sum(selected.values()),
            "verified": verified, "total": len(SUITE_JOB_IDS),
            "fallback_full": fallback_full}


def main() -> int:
    try:
        report = verify(
            os.environ["GITHUB_EVENT_NAME"],
            json.loads(os.environ["TOR_CI_NEEDS_JSON"]),
        )
    except (GateError, KeyError, TypeError, ValueError) as exc:
        print(f"TOR_CI_REQUIRED_BLOCKED: {exc}", file=sys.stderr)
        return 1
    print("TOR_CI_REQUIRED_OK " + json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
