#!/usr/bin/env python3
"""Temporary anonymous, read-only public API shape probe for I-06 PR diagnostics.

Never used for release approval. Does not read a token, SSH or any deployment input.
"""
import json
from urllib.request import Request, urlopen

BASE = "https://api.github.com/repos/calex794/tor/"
HEADERS = {
    "Accept": "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "tor-i06-public-policy-shape-probe",
}
RULESET_ID = 26235453


def get(path):
    with urlopen(Request(BASE + path, headers=HEADERS), timeout=20) as response:
        return json.load(response)


def safe_check_shape(rule):
    if not isinstance(rule, dict):
        return "malformed"
    params = rule.get("parameters")
    if not isinstance(params, dict):
        return {"parameters_type": type(params).__name__}
    checks = params.get("required_status_checks")
    return {
        "parameter_keys": sorted(params),
        "checks": [
            {"context": x.get("context"), "integration_id": x.get("integration_id")}
            for x in checks if isinstance(x, dict)
        ] if isinstance(checks, list) else type(checks).__name__,
    }


def run():
    branch = get("branches/main")
    summary = get("rulesets?includes_parents=true&targets=branch&per_page=100")
    detail = get(f"rulesets/{RULESET_ID}?includes_parents=true")
    effective = get("rules/branches/main?per_page=100")
    print("I06_PUBLIC_API_SHAPE " + json.dumps({
        "main_protected": branch.get("protected"),
        "classic_enabled": branch.get("protection", {}).get("enabled"),
        "ruleset_count": len(summary) if isinstance(summary, list) else "malformed",
        "ruleset_summary": [{
            "id": r.get("id"), "target": r.get("target"),
            "enforcement": r.get("enforcement"),
            "updated_at": r.get("updated_at"),
        } for r in summary] if isinstance(summary, list) else "malformed",
        "detail": {
            "target": detail.get("target"),
            "source": detail.get("source"),
            "enforcement": detail.get("enforcement"),
            "updated_at": detail.get("updated_at"),
            "bypass_field_present": "bypass_actors" in detail,
            "can_bypass_field_present": "current_user_can_bypass" in detail,
            "can_bypass": detail.get("current_user_can_bypass"),
            "types": [r.get("type") for r in detail.get("rules", [])],
            "required": [safe_check_shape(r) for r in detail.get("rules", [])
                         if r.get("type") == "required_status_checks"],
        },
        "effective_count": len(effective) if isinstance(effective, list) else "malformed",
        "effective": [{
            "type": r.get("type"),
            "ruleset_id": r.get("ruleset_id"),
            "ruleset_source_type": r.get("ruleset_source_type"),
            "required": safe_check_shape(r) if r.get("type") == "required_status_checks" else None,
        } for r in effective] if isinstance(effective, list) else "malformed",
    }, sort_keys=True))


if __name__ == "__main__":
    try:
        run()
    except Exception as exc:
        print("I06_PUBLIC_API_SHAPE_UNAVAILABLE: " + type(exc).__name__)
