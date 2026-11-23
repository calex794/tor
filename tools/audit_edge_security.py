#!/usr/bin/env python3
"""AB-EDGE: read-only and secret-free local Nginx security inventory.

Captures nginx -T in memory, emits fixed enums only; no SSH, network probes,
writes or restarts. Examines disk config, not necessarily live worker config.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys

MAX_SIZE = 4 * 1024 * 1024
HOST = "pandaolate.ru"
TLS = re.compile(r"\blisten\s+(?:\[[^]]+\]:)?(?:[0-9.]+:)?443\b[^;]*\bssl\b")
ZONES = re.compile(r"\blimit_req_zone\s+[^;]*\bzone=([a-zA-Z0-9_]+):")
USED = re.compile(r"\blimit_req\s+zone=([a-zA-Z0-9_]+)\b")
REAL = re.compile(r"\bproxy_set_header\s+X-Real-IP\s+([^;]+);", re.I)
ACCESS = re.compile(r"\baccess_log\s+([^;]+);")
SCANNER = re.compile(
    r"\blocation\s+~\*?\s+([^{;]+)\{\s*return\s+(?:403|404|444)\s*;",
    re.I | re.S,
)


def remove_comments(raw: str) -> str:
    output = []
    quote = ""
    escape = comment = False
    for char in raw:
        if comment:
            if char == "\n":
                comment = False
                output.append(char)
            continue
        if quote:
            output.append(char)
            if escape:
                escape = False
            elif char == "\\":
                escape = True
            elif char == quote:
                quote = ""
        elif char in "'\"":
            quote = char
            output.append(char)
        elif char == "#":
            comment = True
        else:
            output.append(char)
    if quote:
        raise ValueError("unclosed Nginx quoted string")
    return "".join(output)


def extract_servers(raw: str) -> list[str]:
    config = remove_comments(raw)
    servers = []
    for match in re.finditer(r"\bserver\s*\{", config):
        cursor, depth, quote, escaped = match.end(), 1, "", False
        while cursor < len(config) and depth:
            char = config[cursor]
            cursor += 1
            if quote:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quote:
                    quote = ""
            elif char in "'\"":
                quote = char
            elif char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
        if depth:
            raise ValueError("unbalanced Nginx server block")
        servers.append(config[match.end():cursor - 1])
    return servers


def audit(raw: str) -> dict:
    if not isinstance(raw, str) or len(raw.encode("utf-8")) > MAX_SIZE:
        raise ValueError("configuration dump too large")
    cleaned = remove_comments(raw)
    selected = []
    for server in extract_servers(raw):
        names = set(" ".join(re.findall(r"\bserver_name\s+([^;]+);", server)).split())
        if HOST in names and TLS.search(server):
            selected.append(server)
    findings = []
    if len(selected) != 1:
        findings.append("TLS_VHOST_COUNT_NOT_ONE")
    if not selected:
        result = dict(source_ip="NOT_VERIFIED", rate_limit="NOT_VERIFIED",
                      access_log="NOT_VERIFIED", scanner="NOT_VERIFIED",
                      backend="NOT_VERIFIED")
    else:
        headers = [v.strip() for server in selected for v in REAL.findall(server)]
        if headers and set(headers) == {"$remote_addr"}:
            source_ip = "DIRECT_PEER_VARIABLE"
        elif headers:
            source_ip = "TRUST_CHAIN_REVIEW"
            findings.append("X_REAL_IP_USES_UNVERIFIED_VALUE")
        else:
            source_ip = "NOT_VERIFIED"
            findings.append("X_REAL_IP_MISSING")
        if re.search(r"\breal_ip_header\s+", cleaned):
            source_ip = "TRUST_CHAIN_REVIEW"
            findings.append("NGINX_REAL_IP_REWRITE_REVIEW")
        zones = set(ZONES.findall(cleaned))
        attached = set().union(*(set(USED.findall(s)) for s in selected))
        if not attached:
            rate = "ABSENT"
            findings.append("RATE_LIMIT_NOT_ATTACHED")
        elif not attached.issubset(zones):
            rate = "NOT_VERIFIED"
            findings.append("RATE_ZONE_NOT_DECLARED")
        else:
            modes = [v.strip() for s in selected
                     for v in re.findall(r"\blimit_req_dry_run\s+([^;]+);", s)]
            if modes and set(modes) == {"on"}:
                rate = "DRY_RUN"
            elif not modes or set(modes) == {"off"}:
                rate = "ENFORCEMENT_CANDIDATE"
            else:
                rate = "MIXED_REVIEW"
                findings.append("RATE_LIMIT_MODE_MIXED")
        values = [v.strip() for s in selected for v in ACCESS.findall(s)]
        if not values:
            logs = "INHERITED_UNVERIFIED"
            findings.append("ACCESS_LOG_INHERITANCE_UNVERIFIED")
        elif all(v == "off" for v in values):
            logs = "OFF"
            findings.append("ACCESS_LOG_DISABLED")
        elif any(v == "off" for v in values):
            logs = "MIXED"
            findings.append("ACCESS_LOG_MIXED")
        else:
            logs = "CONFIGURED_FORMAT_UNREVIEWED"
        scanner = "PRESENT" if any(
            "wp-admin" in pattern.lower()
            for s in selected for pattern in SCANNER.findall(s)
        ) else "NOT_VERIFIED"
        if scanner != "PRESENT":
            findings.append("SCANNER_DENIAL_NOT_VERIFIED")
        backend = "PRESENT_DESTINATION_UNVERIFIED" if any(
            re.search(r"\bproxy_pass\s+[^;]+;", s) for s in selected
        ) else "ABSENT"
        if backend == "ABSENT":
            findings.append("NO_BACKEND_PROXY")
        result = dict(source_ip=source_ip, rate_limit=rate,
                      access_log=logs, scanner=scanner, backend=backend)
    return {
        "schema": "tor.nginx_edge_audit.v1",
        "scope": "nginx -T disk config only; not live traffic or running worker proof",
        "matching_tls_vhosts": min(9, len(selected)),
        **result,
        "tor_auth_trust_nginx_real_ip": "NOT_INSPECTED",
        "direct_backend_exposure": "NOT_INSPECTED",
        "findings": sorted(set(findings)),
    }


def read_nginx_local() -> str:
    try:
        response = subprocess.run(
            ["nginx", "-T"], capture_output=True, timeout=15,
            check=False, text=True, encoding="utf-8", errors="replace"
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ValueError("local Nginx check unavailable") from exc
    if response.returncode != 0:
        raise ValueError("Nginx configuration test failed")
    return response.stdout


def main() -> int:
    argparse.ArgumentParser(description=__doc__).parse_args()
    try:
        report = audit(read_nginx_local())
    except (ValueError, UnicodeError):
        print("TOR_EDGE_AUDIT_UNAVAILABLE: cannot review local nginx configuration",
              file=sys.stderr)
        return 2
    print(json.dumps(report, ensure_ascii=True, sort_keys=True))
    return 0 if report["matching_tls_vhosts"] == 1 else 1


if __name__ == "__main__":
    raise SystemExit(main())
