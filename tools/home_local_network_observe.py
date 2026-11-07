#!/usr/bin/env python3
"""Read-only local TOR network observations. No hosts contacted, no secrets logged.

Runs fixed Linux ip and ss read-only commands on the machine where the
operator explicitly invokes it. Outputs only predefined findings. Does NOT
verify a home router, cloud firewall, real access, TLS, or remote peer.
"""
from __future__ import annotations

import argparse
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import sys

CANDIDATE = ipaddress.ip_network("10.88.0.0/30")
WG_UDP_PORT = 51820
CORE_TCP_PORTS = {3000, 5432, 18081}
REPORT_SCHEMA = "tor.home_local_observation.v1"
COMMANDS = {
    "addresses": ["ip", "-j", "-4", "address", "show"],
    "routes": ["ip", "-j", "-4", "route", "show", "table", "all"],
    "listeners": ["ss", "-H", "-n", "-l", "-t", "-u"],
}


class ObservationError(ValueError):
    """Failure to collect or interpret observations; report no false clearance."""


def run_readonly_commands() -> tuple[object, object, str]:
    results = {}
    for label, command in COMMANDS.items():
        try:
            response = subprocess.run(
                command, capture_output=True, text=True, timeout=8,
                check=True, env={"PATH": os.defpath, "LC_ALL": "C"},
            )
        except (OSError, subprocess.SubprocessError):
            # Exceptions may contain raw private IPs; never print them.
            raise ObservationError("One required read-only command failed.") from None
        if len(response.stdout) > 2_000_000:
            raise ObservationError("The local result exceeds the memory limit.")
        try:
            results[label] = (
                response.stdout
                if label == "listeners"
                else json.loads(response.stdout)
            )
        except json.JSONDecodeError:
            raise ObservationError("A required local command returned invalid JSON.") from None
    return results["addresses"], results["routes"], results["listeners"]


def local_ipv4_networks(addresses: object, routes: object) -> list[ipaddress.IPv4Network]:
    if not isinstance(addresses, list) or not isinstance(routes, list):
        raise ObservationError("Local interface or route format is invalid.")
    networks: list[ipaddress.IPv4Network] = []
    try:
        for interface in addresses:
            if not isinstance(interface, dict):
                raise ValueError
            address_info = interface.get("addr_info")
            if not isinstance(address_info, list):
                raise ValueError
            for item in address_info:
                if not isinstance(item, dict):
                    raise ValueError
                if item.get("family") != "inet":
                    continue
                address = item["local"]
                prefix = item["prefixlen"]
                if not isinstance(address, str) or type(prefix) is not int:
                    raise ValueError
                network = ipaddress.ip_network(f"{address}/{prefix}", strict=False)
                if not isinstance(network, ipaddress.IPv4Network):
                    raise ValueError
                networks.append(network)
        for route in routes:
            if not isinstance(route, dict):
                raise ValueError
            destination = route.get("dst", "default")
            if destination == "default":
                continue
            if not isinstance(destination, str):
                raise ValueError
            network = ipaddress.ip_network(destination, strict=False)
            if not isinstance(network, ipaddress.IPv4Network):
                raise ValueError
            networks.append(network)
    except (KeyError, ValueError, TypeError):
        raise ObservationError("Local network data cannot be safely parsed.") from None
    return networks


def parse_listeners(raw: object) -> list[tuple[str, int, str]]:
    if not isinstance(raw, str):
        raise ObservationError("Socket listener data must be text.")
    listeners = []
    for line in raw.splitlines():
        if not line.strip():
            continue
        parts = line.split()
        if len(parts) < 5 or parts[0].lower() not in {"tcp", "udp"}:
            raise ObservationError("Unexpected socket listener format.")
        protocol = parts[0].lower()
        endpoint = parts[4]
        if ":" not in endpoint:
            raise ObservationError("Socket address does not include a port.")
        host, raw_port = endpoint.rsplit(":", 1)
        if not raw_port.isdigit():
            raise ObservationError("Socket listener port is not numeric.")
        number = int(raw_port)
        if not 0 <= number <= 65535:
            raise ObservationError("Socket listener port is out of range.")
        if host.startswith("[") and host.endswith("]"):
            host = host[1:-1]
        if host.startswith("::ffff:"):
            host = host[7:]
        if not host:
            raise ObservationError("Socket listener address is missing.")
        listeners.append((protocol, number, host))
    return listeners


def loopback(host: str) -> bool:
    try:
        address = ipaddress.ip_address(host)
        if isinstance(address, ipaddress.IPv6Address) and address.ipv4_mapped:
            return address.ipv4_mapped.is_loopback
        return address.is_loopback
    except ValueError:
        return False


def classify(role: str, addresses: object, routes: object, raw_ss: object) -> dict:
    if role not in {"home", "edge"}:
        raise ObservationError("Role must be home or edge.")
    networks = local_ipv4_networks(addresses, routes)
    listeners = parse_listeners(raw_ss)
    candidate_overlap = any(CANDIDATE.overlaps(network) for network in networks)
    occupied_wg_port = any(
        protocol == "udp" and number == WG_UDP_PORT
        for protocol, number, _host in listeners
    )
    core_listeners = [
        (number, host) for protocol, number, host in listeners
        if protocol == "tcp" and number in CORE_TCP_PORTS
    ]
    if role == "home":
        unsafe_bind = any(
            (
                not loopback(host) if number in {3000, 5432}
                else host != "10.88.0.2"
            )
            for number, host in core_listeners
        )
        old_vps_core = False
    else:
        # Previous demo VPS can still have loopback Rust/PG.
        # Observation is NOT permission to remove or reconfigure them.
        unsafe_bind = any(
            not loopback(host) for _number, host in core_listeners
        )
        old_vps_core = bool(core_listeners)
    findings = [
        {"id": "candidate_tunnel_network_overlap", "flagged": candidate_overlap},
        {"id": "candidate_wireguard_udp_port_in_use", "flagged": occupied_wg_port},
        {"id": "unsafe_app_or_pg_listen_address", "flagged": unsafe_bind},
        {"id": "legacy_core_services_present_on_edge", "flagged": old_vps_core},
    ]
    flagged = any(item["flagged"] for item in findings)
    return {
        "schema": REPORT_SCHEMA,
        "role": role,
        "decision": "BLOCKED_REVIEW" if flagged else "MANUAL_REVIEW_REQUIRED",
        "scope": "local read-only observation, not a deployment approval",
        "findings": findings,
        "not_checked": [
            "home router forwarding/NAT and CGNAT",
            "cloud firewall, external access, DNS or TLS certificate",
            "process ownership, WireGuard peer authentication or keys",
            "actual connectivity, route to S3, and ISP throughput",
            "remote host interfaces and firewalls",
        ],
    }


def save_private_new(path: Path, content: str) -> None:
    # Never follow an existing symlink or replace a previous report.
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    flags |= getattr(os, "O_NOFOLLOW", 0)
    handle = os.open(path, flags, 0o600)
    with os.fdopen(handle, "w", encoding="utf-8") as stream:
        stream.write(content)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--role", required=True, choices=("home", "edge"))
    parser.add_argument(
        "--output", type=Path,
        help="Optional new local report file, mode 0600; do not commit it.",
    )
    args = parser.parse_args()
    try:
        report = classify(args.role, *run_readonly_commands())
        rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
        if args.output:
            save_private_new(args.output, rendered)
        else:
            print(rendered, end="")
    except (ObservationError, OSError):
        print("TOR_LOCAL_NETWORK_OBSERVATION_UNAVAILABLE: no clearance issued.",
              file=sys.stderr)
        return 2
    # Never return 0 merely because the host has no conflicting sockets.
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
