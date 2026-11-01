#!/usr/bin/env python3
"""RIL-09: read-only route fingerprint and Nginx HTTPS exposure audit.

Offline CI checks only repository contracts. The optional --nginx-dump and
--origin checks MUST run on the actual VPS to establish external exposure.
No secrets, complete configuration, cookies or HTML responses are printed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import ssl
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INVENTORY = Path("docs/contracts/ril-route-guards-v1.json")
EXPECTED_HOST = "pandaolate.ru"
EXPECTED_UPSTREAM = "http://127.0.0.1:3000"
AUDITED_ADMIN_MAIL = {
    ("GET", "/admin/mail", "admin_mail::index", "admin_mail"),
    ("GET", "/admin/mail/users/{user_id}", "admin_mail::user", "admin_mail"),
    ("GET", "/admin/mail/{id}", "admin_mail::conversation", "admin_mail"),
}
PRIVATE_PROBES = (
    ("/me", {301, 302, 303, 307, 308, 401, 403, 404}),
    ("/api/v1/chapters/1/read", {403, 404}),
    ("/admin", {403, 404}),
    ("/internal/metrics", {403, 404}),
    ("/.env", {403, 404}),
)


def fingerprint(root: Path = ROOT) -> dict:
    data = json.loads((root / INVENTORY).read_text(encoding="utf-8"))
    if data.get("schema") != "tor_ril08_route_policies_v1":
        raise ValueError("wrong route-policy registry version")
    rows = data["routes"]
    keys = sorted(
        (row["method"], row["template"], row["source"], row["handler"], row["policy"])
        for row in rows
    )
    if len(keys) != len(set(keys)):
        raise ValueError("ambiguous security route bindings")
    canonical = json.dumps(keys, ensure_ascii=True, separators=(",", ":"))
    return {
        "version": "ril_exposure_fingerprint_v1",
        "route_bindings": len(keys),
        "route_pairs": len(set((row[0], row[1]) for row in keys)),
        "sha256": hashlib.sha256(canonical.encode("utf-8")).hexdigest(),
    }


def check_repository(root: Path = ROOT) -> list[str]:
    problems = []
    try:
        data = json.loads((root / INVENTORY).read_text(encoding="utf-8"))
        routes = data["routes"]
        fingerprint(root)
        unit = (root / "ops/tor-web.service.example").read_text(encoding="utf-8")
        main = (root / "crates/web/src/main.rs").read_text(encoding="utf-8")
    except (OSError, ValueError, KeyError, TypeError) as exc:
        return [f"missing RIL exposure input: {exc}"]
    if "Environment=TOR_LISTEN_ADDR=127.0.0.1:3000" not in unit:
        problems.append("example TOR service must use loopback, never a public listener")
    if '"127.0.0.1:3000"' not in main:
        problems.append("default TOR listener is not loopback")
    if "Environment=TOR_AUTH_ENABLED=1" not in unit:
        problems.append("example production unit must explicitly enable auth")
    if "TOR_RIL_SINGLE_INSTANCE=1" in unit:
        problems.append("sample systemd must not enable personal path analysis by default")
    for row in routes:
        template = row["template"]
        policy = row["policy"]
        if template.startswith(("/admin", "/internal")) and policy != "deny":
            key = (row["method"], template, row["handler"], policy)
            if row["source"] != "crates/web/src/auth.rs" or key not in AUDITED_ADMIN_MAIL:
                problems.append(f"internal/admin route is not denied: {row['method']} {template}")
        if template == "/api/v1/chapters/{id}/read" and policy != "deny":
            problems.append("unconnected reader API must remain disabled")
    return problems


def _nginx_servers(config: str) -> list[str]:
    # The nginx -T output may contain other vhosts. Parse only server blocks,
    # respecting nested location braces; never report full configuration.
    config = "\n".join(
        line.split("#", 1)[0] for line in config.splitlines()
    )
    servers = []
    for found in re.finditer(r"(?m)^\s*server\s*\{", config):
        start = found.end()
        depth = 1
        quoted = False
        escaped = False
        i = start
        while i < len(config) and depth:
            ch = config[i]
            i += 1
            if quoted:
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch in ("'", '"'):
                    quoted = False
            elif ch in ("'", '"'):
                quoted = True
            elif ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
        if depth == 0:
            servers.append(config[start:i - 1])
    return servers


def check_nginx(config: str, host: str = EXPECTED_HOST) -> list[str]:
    findings: list[str] = []
    servers = []
    for source in _nginx_servers(config):
        matches = re.findall(r"\bserver_name\s+([^;]+);", source)
        names = set(" ".join(matches).split())
        if host in names:
            servers.append(source)
    if not servers:
        return ["no Nginx server_name matching audited host"]
    https = [
        s for s in servers if re.search(
            r"\blisten\s+(?:\[[^]]+\]:)?443\b[^;]*\bssl\b", s
        )
    ]
    if not https:
        findings.append("audited host has no explicit HTTPS ssl listener")
    for index, source in enumerate(https):
        label = f"HTTPS server #{index + 1}"
        passes = re.findall(r"\bproxy_pass\s+([^;]+);", source)
        if not passes:
            findings.append(f"{label}: no trusted TOR upstream configured")
        if EXPECTED_UPSTREAM not in passes:
            findings.append(f"{label}: expected loopback TOR upstream missing")
        if any(endpoint != EXPECTED_UPSTREAM for endpoint in passes):
            findings.append(f"{label}: unreviewed proxy_pass destination")
        if re.search(r"\bautoindex\s+on\s*;", source):
            findings.append(f"{label}: directory listing enabled")
        if re.search(r"\bstub_status\s+on\s*;", source):
            findings.append(f"{label}: raw Nginx status exposed on public host")
        if re.search(r"\b(?:root|alias)\s+/(?:etc|proc|run|root)(?:/|;)", source):
            findings.append(f"{label}: sensitive system filesystem publication")
    for source in servers:
        if re.search(r"\blisten\s+(?:\[[^]]+\]:)?80\b", source):
            if re.search(r"\bproxy_pass\s+", source):
                findings.append("HTTP port 80 proxies authenticated requests without HTTPS")
    return findings


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


def probe(origin: str) -> tuple[dict[str, int], list[str]]:
    if origin != f"https://{EXPECTED_HOST}":
        return {}, ["public origin must equal the approved TOR HTTPS host"]
    opener = urllib.request.build_opener(_NoRedirect(), urllib.request.HTTPSHandler(
        context=ssl.create_default_context()
    ))
    checks = [("/", {200}), *PRIVATE_PROBES]
    statuses: dict[str, int] = {}
    errors = []
    for path, allowed in checks:
        request = urllib.request.Request(
            origin + path, headers={"User-Agent": "TOR-RIL09-readonly-audit/1"}
        )
        try:
            with opener.open(request, timeout=8) as response:
                status = response.status
        except urllib.error.HTTPError as exc:
            status = exc.code
        except (urllib.error.URLError, TimeoutError, OSError):
            errors.append(f"{path}: cannot verify public HTTPS response")
            continue
        statuses[path] = status
        if status not in allowed:
            errors.append(f"{path}: unsafe/unexpected anonymous response {status}")
    return statuses, errors


def run(root: Path, config: str | None = None, origin: str | None = None) -> tuple[dict, list[str]]:
    report = fingerprint(root)
    errors = check_repository(root)
    if config is not None:
        errors.extend(check_nginx(config))
        report["nginx_config_reviewed"] = True
    else:
        report["nginx_config_reviewed"] = False
    if origin:
        statuses, findings = probe(origin)
        report["anonymous_statuses"] = statuses
        errors.extend(findings)
    report["public_origin_reviewed"] = bool(origin)
    report["findings_count"] = len(errors)
    return report, errors


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--nginx-dump", type=Path, help="restricted copy of nginx -T from VPS")
    cli.add_argument("--origin", help="read-only anonymous HTTPS probes")
    args = cli.parse_args()
    try:
        content = args.nginx_dump.read_text(encoding="utf-8") if args.nginx_dump else None
        report, issues = run(ROOT, content, args.origin)
    except (OSError, ValueError) as exc:
        print("FAIL:", str(exc), file=sys.stderr)
        raise SystemExit(1)
    # Never print the raw Nginx dump, session headers or response bodies.
    print(json.dumps(report, ensure_ascii=True, sort_keys=True))
    for issue in issues:
        print("FAIL:", issue, file=sys.stderr)
    if issues:
        raise SystemExit(1)
