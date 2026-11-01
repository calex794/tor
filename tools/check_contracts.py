#!/usr/bin/env python3
"""Protect prelaunch contracts against semantic regressions (stdlib, no services)."""
from __future__ import annotations

import csv
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONTRACTS = (
    "paid-delivery-v1.md",
    "ril-platform-v1.md",
    "ril-reader-signals-v1.md",
    "finance-ad-exact-v1.md",
    "refunds-payouts-v1.md",
    "recommendations-golden-v1.md",
    "import-storage-v1.md",
    "events-analytics-v1.md",
    "api-evolution-v1.md",
    "release-profile-v1.md",
)
HIGH_RISK_PATHS = (
    "/api/v1/reader/chapters/{chapter_id}/delivery-grants",
    "/api/v1/chapters/{id}/read",
    "/api/v1/purchases",
    "/api/v1/purchases/{id}/refund-request",
    "/api/v1/me/payout-requests",
    "/api/v1/ads/daily-campaigns",
    "/api/v1/ads/chapter/campaigns",
    "/api/v1/ads/chapter/impressions",
    "/api/v1/books/{id}/recommendations",
    "/api/v1/import-jobs",
    "/api/v1/uploads/prepare",
    "/api/v1/uploads/finalize",
)
MONEY_FLAGS = (
    "paid_delivery_enabled", "wallet_purchase_enabled",
    "boosty_funding_enabled", "cash_out_enabled",
    "legacy_ads_purchase_enabled", "chapter_ads_purchase_enabled",
    "chapter_ads_serving_enabled", "chapter_ads_billable_enabled",
    "chapter_ads_settlement_enabled", "personalized_recommendations_enabled",
    "s3_uploads_enabled",
)


def _load_json(path: Path, errors: list[str]) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        errors.append(f"invalid or missing {path.name}: {exc}")
        return {}
    if not isinstance(value, dict):
        errors.append(f"{path.name} root must be a JSON object")
        return {}
    return value


def _all_refs(item: object):
    if isinstance(item, dict):
        for key, value in item.items():
            if key == "$ref" and isinstance(value, str):
                yield value
            else:
                yield from _all_refs(value)
    elif isinstance(item, list):
        for value in item:
            yield from _all_refs(value)


def _scenario(root: Path, code: str, errors: list[str]):
    file = root / "docs" / "roadmap" / "scenarios.tsv"
    try:
        with file.open(encoding="utf-8", newline="") as stream:
            rows = csv.DictReader((x for x in stream if not x.startswith("#")), delimiter="\t")
            return next((r for r in rows if r.get("code") == code), {})
    except (OSError, ValueError, StopIteration) as exc:
        errors.append(f"bad scenarios.tsv: {exc}")
        return {}


def _golden_score(case: dict, errors: list[str]):
    case_id = case.get("id", "")
    weights = case.get("weights", {})
    vals = [weights.get(x, 0) for x in ("behavioral", "content", "popularity")]
    expected = case.get("expected", {})
    if not all(isinstance(x, int) and x >= 0 for x in vals) or sum(vals) != 100:
        if expected.get("rejected_reason") != "invalid_weights":
            errors.append(f"{case_id}: invalid weights must reject")
        return
    winning: dict[tuple[str, str], int] = {}
    for candidate in case.get("candidates", []):
        source = candidate["source"]
        if source == "related_books":
            continue
        score = candidate.get("normalized_score", 0)
        if not isinstance(score, int) or score <= 0:
            errors.append(f"{case_id}: golden candidates must have positive normalized scores")
            return
        contribution = score * weights[source] // 100
        key = (candidate["book_id"], source)
        winning[key] = max(winning.get(key, 0), contribution)
    totals: dict[str, int] = {}
    for (book_id, _), value in winning.items():
        totals[book_id] = totals.get(book_id, 0) + value
    for book_id, quality in case.get("quality_score_by_book_id", {}).items():
        totals[book_id] = totals.get(book_id, 0) + min(50, max(0, min(1000, quality)) // 20)
    got = [
        {"book_id": book_id, "final_score": min(1000, score)}
        for book_id, score in sorted(totals.items(), key=lambda x: (-x[1], x[0]))
        if score > 0
    ][:case.get("limit", 20)]
    if got != expected.get("ordered"):
        errors.append(f"{case_id}: scoring oracle mismatch expected={expected.get('ordered')} got={got}")


def check(root: Path = ROOT) -> list[str]:
    errors: list[str] = []
    base = root / "docs" / "contracts"
    for filename in CONTRACTS:
        path = base / filename
        if not path.is_file() or path.stat().st_size < 300:
            errors.append(f"missing executable prelaunch contract {filename}")

    spec = _load_json(base / "openapi-v1.json", errors)
    if spec:
        if spec.get("openapi") != "3.1.0":
            errors.append("OpenAPI must be 3.1.0")
        paths = spec.get("paths", {})
        for url in HIGH_RISK_PATHS:
            if url not in paths:
                errors.append(f"missing high-risk API: {url}")
        defs = spec.get("components", {})
        if not isinstance(defs, dict):
            errors.append("components must be an object")
            defs = {}
        for ref in _all_refs(spec):
            if not ref.startswith("#/components/"):
                errors.append(f"unsupported OpenAPI reference {ref}")
                continue
            node: object = spec
            for segment in ref.split("/")[1:]:
                if not isinstance(node, dict) or segment not in node:
                    errors.append(f"unresolved OpenAPI reference {ref}")
                    break
                node = node[segment]
        schemas = defs.get("schemas", {})
        if "body_markdown" in schemas.get("ChapterMetadata", {}).get("properties", {}):
            errors.append("paid chapter response reintroduced plaintext body_markdown")
        imports = schemas.get("ImportCreate", {})
        if "source_url" in imports.get("properties", {}):
            errors.append("frozen crawler source_url reintroduced in ImportCreate")
        if imports.get("properties", {}).get("mode", {}).get("const") != "upload":
            errors.append("frozen crawler mode=url reintroduced")
        for name in ("PurchaseReceipt", "DeliveryGrant", "RecommendationPage", "AdImpression"):
            if name not in schemas:
                errors.append(f"missing required DTO {name}")
        for url,method in (
            ("/api/v1/purchases","post"),
            ("/api/v1/ads/daily-campaigns","post"),
            ("/api/v1/ads/chapter/campaigns","post"),
            ("/api/v1/me/payout-requests","post"),
            ("/api/v1/purchases/{id}/refund-request","post"),
        ):
            op = paths.get(url, {}).get(method, {})
            if not any(p.get("name") == "Idempotency-Key"
                       and p.get("schema", {}).get("minLength") == 43
                       and p.get("schema", {}).get("maxLength") == 43
                       for p in op.get("parameters", [])):
                errors.append(f"{url}: missing strict Idempotency-Key header")
        for path in paths:
            if "/crawler" in path or "/import-by-url" in path:
                errors.append("frozen crawler API exists in OpenAPI")

    flags = _load_json(base / "release-flags-v1.json", errors)
    values = flags.get("flags", {})
    if flags:
        for name in MONEY_FLAGS:
            if name not in values or values[name].get("default_enabled") is not False:
                errors.append(f"{name}: must default disabled until release proof")
        for name in ("recommendations_enabled", "recommendation_attribution_enabled",
                     "organic_discovery_attribution_enabled"):
            if values.get(name, {}).get("default_enabled") is not False:
                errors.append(f"{name}: must default disabled until exact-head proof")
        organic = values.get("organic_discovery_attribution_enabled", {})
        if set(organic.get("requires", [])) != {
                "recommendations_enabled", "recommendation_attribution_enabled"}:
            errors.append("organic discovery must require both REC and attribution gates")
        if "TOR_ORGANIC_ATTRIBUTION_ENABLED" not in organic.get("gate", ""):
            errors.append("organic discovery flag must name its actual runtime environment gate")
        if values.get("recommendations_public_fallback_enabled", {}).get("default_enabled") is not True:
            errors.append("safe public recommendations fallback must default true")
        if "url_import" not in flags.get("forbidden_runtime_features", []):
            errors.append("frozen URL-import must remain unsupported, not a runtime flag")
        for name, config in values.items():
            for dependency in config.get("requires", []):
                if dependency not in values:
                    errors.append(f"{name}: unknown dependent flag {dependency}")

    oracle = _load_json(base / "rec-golden-v1.json", errors)
    if oracle:
        if oracle.get("schema") != "tor.rec_golden.v1":
            errors.append("wrong recommendation golden schema")
        cases = oracle.get("test_cases", [])
        if {c.get("id") for c in cases} != {"REC-M01", "REC-M02", "REC-M03", "REC-M08"}:
            errors.append("required rec oracle cases missing")
        for case in cases:
            _golden_score(case, errors)
        if len(oracle.get("system_acceptance_cases", [])) < 8:
            errors.append("missing external RIL/PG/S3 acceptance cases")

    # RIL is system-wide: every stage must have an accountable existing task.
    ril_map = {
        "I-02": ("RIL-01", "RIL-02"),
        "I-13": ("RIL-03",),
        "A-09": ("RIL-04",),
        "G-06": ("RIL-05",),
        "G-07": ("RIL-06",),
        "G-10": ("RIL-07",),
        "I-07": ("RIL-08", "RIL-09"),
        "I-09": ("RIL-10",),
        "F-08": ("RIL-11",),
        "F-07": ("RIL-12",),
        "F-10": ("RIL-12",),
        "H-07": ("RIL-13",),
        "I-12": ("RIL-14",),
    }
    ril_contract = base / "ril-platform-v1.md"
    try:
        ril_text = ril_contract.read_text(encoding="utf-8")
    except OSError:
        errors.append("missing RIL platform contract")
    else:
        for stage in (f"RIL-{i:02d}" for i in range(1, 15)):
            if stage not in ril_text:
                errors.append(f"RIL platform missing stage {stage}")
        for heading in ("RIL Core", "Behavior Intelligence", "Route Governance",
                        "Route Cost Map", "PG hourly/daily", "## 7."):
            if heading not in ril_text:
                errors.append(f"RIL platform missing system section {heading}")
    try:
        with (root / "docs" / "roadmap" / "tasks.tsv").open(encoding="utf-8", newline="") as stream:
            ril_tasks = list(csv.DictReader((row for row in stream if not row.startswith("#")), delimiter="\t"))
    except (OSError, ValueError) as exc:
        errors.append(f"RIL task map unreadable: {exc}")
    else:
        indexed = {row.get("id"): row for row in ril_tasks}
        if len(ril_tasks) != 117 or len(indexed) != 117:
            errors.append("RIL task map must preserve 117 unique tasks")
        for task, stages in ril_map.items():
            row = indexed.get(task, {})
            for stage in stages:
                if stage not in row.get("implementation", ""):
                    errors.append(f"RIL task {task} missing implementation {stage}")
                if stage not in row.get("acceptance", ""):
                    errors.append(f"RIL task {task} missing acceptance {stage}")

    scenario = _scenario(root, "S024", errors)
    if scenario.get("owner") != "-" or scenario.get("decision") != "заморожено" or scenario.get("rust_status") != "Исключено":
        errors.append("S024 crawler must be frozen and excluded from TOR")
    tasks_file = root / "docs" / "roadmap" / "tasks.tsv"
    try:
        with tasks_file.open(encoding="utf-8", newline="") as f:
            tasks = list(csv.DictReader((r for r in f if not r.startswith("#")), delimiter="\t"))
        g03 = next((t for t in tasks if t.get("id") == "G-03"), {})
        if g03.get("day") != "ЗАМОРОЖЕНО" or "не создавать" not in g03.get("implementation", "").lower():
            errors.append("G-03 archived crawler task was reactivated")
    except OSError as exc:
        errors.append(f"bad tasks.tsv: {exc}")
    return errors


if __name__ == "__main__":
    issues = check()
    for issue in issues:
        print("FAIL:", issue, file=sys.stderr)
    if issues:
        raise SystemExit(1)
    print("TOR prelaunch contracts: 61 F, 69 S (61 assigned, 8 frozen/excluded), "
          "OpenAPI, RIL, finance, S3, rec and release gates PASS")
