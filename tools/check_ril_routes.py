#!/usr/bin/env python3
"""Fail closed when TOR Axum routes drift from the bounded RIL taxonomy.

This static check intentionally covers *registered literal* route paths and
methods, including conditional auth routers; it is not an ACL/CSRF audit.
Use a full parser if routing starts using macros or dynamic registration.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CLASSIFIERS = (
    Path("crates/route_intelligence/src/domain/mod.rs"),
    Path("crates/route_intelligence/src/domain/routes.rs"),
    Path("crates/route_intelligence/src/domain/routes/classifier.rs"),
)
_METHODS = r"get|post|put|patch|delete|head|options|trace"


def _production(source: str) -> str:
    # Inline tests construct ad-hoc routers; these are not application routes.
    return source.split("#[cfg(test)]", 1)[0]


def _literal_routes(source: str, filename: str) -> tuple[set[tuple[str, str]], list[str]]:
    routes: set[tuple[str, str]] = set()
    problems: list[str] = []
    for found in re.finditer(r"\.route\s*\(", source):
        start = found.end()
        depth = 1
        quoted = False
        escaped = False
        index = start
        while index < len(source) and depth:
            ch = source[index]
            if quoted:
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch == '"':
                    quoted = False
            elif ch == '"':
                quoted = True
            elif ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
            index += 1
        if depth:
            problems.append(f"{filename}: unterminated .route call")
            continue
        call = source[start:index - 1]
        args = re.match(r'\s*"([^"\n]+)"\s*,(.*)\Z', call, re.S)
        if not args:
            problems.append(f"{filename}: dynamic or non-literal .route call")
            continue
        path, handler = args.groups()
        methods = {m.upper() for m in re.findall(rf"(?<!\w)(?:routing::)?({_METHODS})\s*\(", handler)}
        if not methods:
            problems.append(f"{filename}: no recognized HTTP method for {path}")
        for method in methods:
            routes.add((path, method))
    if re.search(r"\.(?:nest|route_service|nest_service)\s*\(", source):
        problems.append(f"{filename}: nested/service route registration needs explicit RIL audit")
    return routes, problems


def _reference(source: str) -> tuple[set[tuple[str, str]], set[tuple[str, str]], list[str]]:
    registered = re.findall(
        r'RouteRegistration::new\(\s*"([^"]+)"\s*,\s*"([A-Z]+)"\s*,',
        source,
        re.S,
    )
    matches = re.findall(
        r'\(\s*Some\(\s*"([^"]+)"\s*\)\s*,\s*"([A-Z]+)"\s*,?\s*\)\s*=>\s*Some\(\d+\)',
        source,
        re.S,
    )
    issues = []
    if len(registered) != len(set(registered)):
        issues.append("RIL taxonomy contains duplicate route+method registrations")
    if len(matches) != len(set(matches)):
        issues.append("RIL classifier contains duplicate route+method patterns")
    return set(registered), set(matches), issues


def check(root: Path = ROOT) -> list[str]:
    problems: list[str] = []
    try:
        source = "\n".join((root / src).read_text(encoding="utf-8") for src in CLASSIFIERS)
        registered, classifier, issues = _reference(source)
    except OSError as exc:
        return [f"missing RIL classifier registry: {exc}"]
    problems.extend(issues)
    actual: set[tuple[str, str]] = set()
    sources = list((root / "crates").glob("*/src/**/*.rs"))
    if not sources:
        return problems + ["no TOR Rust route sources found"]
    for source in sorted(sources):
        text = _production(source.read_text(encoding="utf-8"))
        found, issues = _literal_routes(text, source.relative_to(root).as_posix())
        actual |= found
        problems.extend(issues)
    if not actual:
        problems.append("no registered Axum routes found")
    for route in sorted(actual - registered):
        problems.append(f"missing RIL taxonomy for Axum route: {route[1]} {route[0]}")
    for route in sorted(registered - actual):
        problems.append(f"stale RIL taxonomy without Axum route: {route[1]} {route[0]}")
    for route in sorted(registered - classifier):
        problems.append(f"RIL classifier does not cover taxonomy: {route[1]} {route[0]}")
    for route in sorted(classifier - registered):
        problems.append(f"RIL classifier contains unregistered taxonomy: {route[1]} {route[0]}")
    return problems


if __name__ == "__main__":
    errors = check()
    for error in errors:
        print("FAIL:", error, file=sys.stderr)
    if errors:
        raise SystemExit(1)
    print("TOR RIL governance: registered Axum routes match taxonomy and classifier")
