"""I-06: fail-closed read-only enforcement evidence for TOR's protected main.

GitHub's public ruleset detail may hide bypass_actors from read-only tokens.
When hidden, permit ONLY the administrator-reviewed, version-pinned ruleset.
Any settings change invalidates the pin and requires an admin re-review.
"""
from __future__ import annotations

from fnmatch import fnmatchcase

REPO = "calex794/tor"
APP_ID = 15368  # GitHub Actions; no "any app" fallback
REQUIRED_CHECKS = {"rules", "ci-required"}
REQUIRED_RULES = {"pull_request", "required_status_checks", "deletion", "non_fast_forward"}

# Reviewed at https://github.com/calex794/tor/rules/26235453 (no bypass list).
# The read-only REST response omits bypass_actors: pin updated_at so any future
# policy edit blocks releases until the admin inspects bypasses again.
REVIEWED_RULESET_ID = 26235453
REVIEWED_RULESET_UPDATED = "2026-10-31T00:28:58.370+05:00"


def _ref_matches(ref: str) -> bool:
    return ref in {"~DEFAULT_BRANCH", "~ALL", "refs/heads/main"}


def _excludes_main(value: str) -> bool:
    return value in {"~DEFAULT_BRANCH", "~ALL"} or any(
        fnmatchcase(ref, value) for ref in ("refs/heads/main", "main")
    )


def _applies_to_main(rule: dict) -> bool:
    cond = rule.get("conditions")
    if not isinstance(cond, dict):
        return False
    ref = cond.get("ref_name")
    if (not isinstance(ref, dict) or not isinstance(ref.get("include"), list)
            or not isinstance(ref.get("exclude"), list)
            or not all(isinstance(x, str) for x in ref["include"] + ref["exclude"])):
        return False
    if not any(_ref_matches(x) for x in ref["include"]):
        return False
    if any(_excludes_main(x) for x in ref["exclude"]):
        return False
    source = rule.get("source_type")
    if source == "Repository":
        return rule.get("source") == REPO
    if source != "Organization" or rule.get("source") != REPO.split("/")[0]:
        return False
    # Organization rules must additionally include this repository.
    names = cond.get("repository_name")
    if (not isinstance(names, dict) or not isinstance(names.get("include"), list)
            or not isinstance(names.get("exclude"), list)
            or not all(isinstance(x, str) for x in names["include"] + names["exclude"])):
        return False
    if not any(x == "~ALL" or fnmatchcase("tor", x) for x in names["include"]):
        return False
    return not any(x == "~ALL" or fnmatchcase("tor", x) for x in names["exclude"])


def _actions_checks(items: object, provider_key: str) -> bool:
    if not isinstance(items, list):
        return False
    found = set()
    for row in items:
        if (not isinstance(row, dict) or not isinstance(row.get("context"), str)
                or not isinstance(row.get(provider_key), int)):
            return False
        if row["context"] in REQUIRED_CHECKS:
            if row[provider_key] != APP_ID:
                return False
            found.add(row["context"])
    return found == REQUIRED_CHECKS


def _classic_is_enforced(branch: dict) -> bool:
    policy = branch.get("protection")
    # The compact branches/main response cannot prove PR-only merge,
    # administrator enforcement or force-push/deletion restrictions. Never
    # infer them from protected=true or a successful status check alone.
    if not isinstance(policy, dict) or policy.get("enabled") is not True:
        return False
    status = policy.get("required_status_checks")
    return (
        policy.get("enforce_admins", {}).get("enabled") is True
        and isinstance(policy.get("required_pull_request_reviews"), dict)
        and policy.get("allow_force_pushes", {}).get("enabled") is False
        and policy.get("allow_deletions", {}).get("enabled") is False
        and isinstance(status, dict)
        and status.get("enforcement_level") == "everyone"
        and _actions_checks(status.get("checks"), "app_id")
    )


def _ruleset_is_enforced(item: dict, effective_rules: list[dict]) -> bool:
    if (item.get("target") != "branch" or item.get("enforcement") != "active"
            or not isinstance(item.get("id"), int)
            or not _applies_to_main(item)
            or item.get("current_user_can_bypass") != "never"):
        return False

    # GitHub hides this field from tokens without write rights to rulesets.
    # Absence is not evidence of an empty bypass list: require reviewed version.
    bypass = item.get("bypass_actors")
    if bypass is not None:
        if bypass != []:
            return False
    elif (item["id"] != REVIEWED_RULESET_ID
          or item.get("updated_at") != REVIEWED_RULESET_UPDATED
          or item.get("source_type") != "Repository"):
        return False

    rows = item.get("rules")
    if not isinstance(rows, list) or not all(isinstance(r, dict) for r in rows):
        return False
    types = [r.get("type") for r in rows]
    if any(types.count(name) != 1 for name in REQUIRED_RULES):
        return False
    status = next(r for r in rows if r["type"] == "required_status_checks")
    parameters = status.get("parameters")
    if (not isinstance(parameters, dict)
            or parameters.get("do_not_enforce_on_create") is True
            or not _actions_checks(parameters.get("required_status_checks"),
                                   "integration_id")):
        return False
    # Verify actual effective branch rules, not just a plausible ruleset name.
    applied = [
        row for row in effective_rules
        if isinstance(row, dict) and row.get("ruleset_id") == item["id"]
    ]
    for name in REQUIRED_RULES:
        if sum(row.get("type") == name for row in applied) != 1:
            return False
    effective_checks = next(r for r in applied if r.get("type") == "required_status_checks")
    effective_parameters = effective_checks.get("parameters")
    return (isinstance(effective_parameters, dict)
            and _actions_checks(effective_parameters.get("required_status_checks"),
                                "integration_id"))


def evaluate_branch_policy(
    branch: dict, rulesets: list[dict] | None, effective_rules: list[dict] | None,
) -> tuple[str, str]:
    """Read-only proof of actual mandatory checks on protected main.

    A classic protection configuration is a separate valid route. Rulesets are
    verified against the active, effective rule inventory for *main*. Missing
    API evidence, changed rules, bypasses or check providers fail closed.
    """
    if not isinstance(branch, dict) or branch.get("name") != "main":
        return "fail", "missing or unexpected main branch metadata"
    if branch.get("protected") is not True:
        return "fail", "main is unprotected"
    if (not isinstance(rulesets, list) or not isinstance(effective_rules, list)
            or any(not isinstance(x, dict) for x in rulesets + effective_rules)):
        return "fail", "missing or malformed effective GitHub rulesets"
    if len(rulesets) >= 100 or len(effective_rules) >= 100:
        return "fail", "ruleset pagination cannot be verified"
    if _classic_is_enforced(branch):
        return "pass", "classic main protection requires both Actions checks for admins"
    reasons = []
    for item in rulesets:
        if _ruleset_is_enforced(item, effective_rules):
            return "pass", f"active effective ruleset #{item['id']} enforces main"
        if item.get("enforcement") != "active":
            reasons.append("inactive ruleset")
        elif not _applies_to_main(item):
            reasons.append("ruleset scope does not include main")
        elif item.get("current_user_can_bypass") != "never":
            reasons.append("bypass not disproved")
        elif item.get("bypass_actors") is None and (
            item.get("id") != REVIEWED_RULESET_ID
            or item.get("updated_at") != REVIEWED_RULESET_UPDATED
        ):
            reasons.append("unreviewed ruleset version or hidden bypass")
        else:
            reasons.append("ruleset/effective-rule evidence mismatch")
    return "fail", ("no verified active main policy: " + ", ".join(sorted(set(reasons)))
                    if reasons else "no verified active main policy: no active rulesets")
