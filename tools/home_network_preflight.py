#!/usr/bin/env python3
"""Read-only, offline TOR home-ingress network preflight.

Consumes an operator-created *sanitized* JSON snapshot. Does not probe any host,
read system networking state, resolve DNS, use credentials or change services.
A passing example is not evidence that the actual home network was inspected.
"""
from __future__ import annotations

import argparse
import ipaddress
import json
from pathlib import Path
import sys

SCHEMA = "tor.home_ingress_preflight.v1"
REQUIRED = {
    "schema",
    "tunnel_network",
    "edge_tunnel_ip",
    "home_tunnel_ip",
    "other_networks",
    "edge_wg_udp_port",
    "edge_busy_udp_ports",
    "edge_tcp_service_owners",
    "edge_peer_allowed_ips",
    "home_peer_allowed_ips",
    "home_initiates_tunnel",
    "home_default_route_through_tunnel",
    "home_router_to_tor_port_forwards",
    "home_service_bind_candidates",
}
EXPECTED_HOME_BINDINGS = {
    "tor-web": ["127.0.0.1:3000"],
    "postgresql": ["127.0.0.1:5432"],
    "home-nginx": ["10.88.0.2:18081"],
}


class SnapshotError(ValueError):
    pass


def require_object(value: object, keys: set[str]) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise SnapshotError("Snapshot has missing, extra, or invalid fields.")
    return value


def port(value: object) -> int:
    if type(value) is not int or not 1 <= value <= 65535:
        raise SnapshotError("Invalid port; expected an integer from 1 to 65535.")
    return value


def ipv4(value: object) -> ipaddress.IPv4Address:
    if not isinstance(value, str):
        raise SnapshotError("Invalid IPv4 address.")
    try:
        address = ipaddress.ip_address(value)
    except ValueError as exc:
        raise SnapshotError("Invalid IPv4 address.") from exc
    if not isinstance(address, ipaddress.IPv4Address):
        raise SnapshotError("Only IPv4 is supported by this snapshot schema.")
    return address


def cidr(value: object) -> ipaddress.IPv4Network:
    if not isinstance(value, str):
        raise SnapshotError("Invalid IPv4 CIDR.")
    try:
        network = ipaddress.ip_network(value, strict=True)
    except ValueError as exc:
        raise SnapshotError("Invalid IPv4 CIDR.") from exc
    if not isinstance(network, ipaddress.IPv4Network):
        raise SnapshotError("Only IPv4 networks are supported.")
    return network


def string_list(value: object) -> list[str]:
    if not isinstance(value, list) or any(not isinstance(v, str) for v in value):
        raise SnapshotError("Expected a list of strings.")
    return value


def port_list(value: object) -> list[int]:
    if not isinstance(value, list):
        raise SnapshotError("Expected a list of ports.")
    values = [port(v) for v in value]
    if len(values) != len(set(values)):
        raise SnapshotError("Duplicate ports are not allowed.")
    return values


def check(snapshot: object) -> dict:
    data = require_object(snapshot, REQUIRED)
    if data["schema"] != SCHEMA:
        raise SnapshotError("Unsupported schema version.")

    network = cidr(data["tunnel_network"])
    edge = ipv4(data["edge_tunnel_ip"])
    home = ipv4(data["home_tunnel_ip"])
    if not isinstance(data["other_networks"], list):
        raise SnapshotError("other_networks must be a list.")
    other = []
    for record in data["other_networks"]:
        item = require_object(record, {"role", "cidr"})
        if item["role"] not in {"home_lan", "vps_lan", "vpn", "docker", "other"}:
            raise SnapshotError("Unknown network role.")
        other.append(cidr(item["cidr"]))

    wg_port = port(data["edge_wg_udp_port"])
    busy = port_list(data["edge_busy_udp_ports"])
    owners = data["edge_tcp_service_owners"]
    if not isinstance(owners, dict) or set(owners) != {"80", "443"}:
        raise SnapshotError("The VPS must declare both public TCP port owners.")
    if any(not isinstance(v, str) for v in owners.values()):
        raise SnapshotError("Invalid edge TCP port owner.")

    edge_routes = string_list(data["edge_peer_allowed_ips"])
    home_routes = string_list(data["home_peer_allowed_ips"])
    # Parse and verify exact /32 routes, not 0.0.0.0/0 or broad LAN networks.
    parsed_edge_routes = [cidr(v) for v in edge_routes]
    parsed_home_routes = [cidr(v) for v in home_routes]
    if type(data["home_initiates_tunnel"]) is not bool:
        raise SnapshotError("home_initiates_tunnel must be true or false.")
    if type(data["home_default_route_through_tunnel"]) is not bool:
        raise SnapshotError("home_default_route_through_tunnel must be boolean.")

    forwards = port_list(data["home_router_to_tor_port_forwards"])
    bindings = data["home_service_bind_candidates"]
    if not isinstance(bindings, dict) or set(bindings) != set(EXPECTED_HOME_BINDINGS):
        raise SnapshotError("Expected exactly the TOR, PostgreSQL and home-Nginx bindings.")
    for values in bindings.values():
        string_list(values)

    results = [
        {
            "id": "tunnel_addresses",
            "ok": (
                network.prefixlen == 30
                and edge == ipaddress.ip_address("10.88.0.1")
                and home == ipaddress.ip_address("10.88.0.2")
                and edge in network.hosts()
                and home in network.hosts()
                and edge != home
            ),
            "meaning": "Tunnel subnet and its two endpoints match the reviewed examples.",
        },
        {
            "id": "route_overlap",
            "ok": not any(network.overlaps(existing) for existing in other),
            "meaning": "The candidate tunnel network does not overlap declared LAN/VPN ranges.",
        },
        {
            "id": "wireguard_port",
            "ok": wg_port == 51820 and wg_port not in busy,
            "meaning": "The planned UDP port is not occupied by a declared existing service.",
        },
        {
            "id": "public_tls_owner",
            "ok": owners == {"80": "nginx", "443": "nginx"},
            "meaning": "The existing HTTPS and HTTP port owners are accounted for.",
        },
        {
            "id": "narrow_peer_routes",
            "ok": (
                parsed_edge_routes == [ipaddress.ip_network("10.88.0.2/32")]
                and parsed_home_routes == [ipaddress.ip_network("10.88.0.1/32")]
            ),
            "meaning": "WireGuard peers only route their opposite tunnel address.",
        },
        {
            "id": "home_outbound",
            "ok": (
                data["home_initiates_tunnel"]
                and not data["home_default_route_through_tunnel"]
            ),
            "meaning": "Home initiates WireGuard and keeps its default internet route.",
        },
        {
            "id": "router_exposure",
            "ok": not forwards,
            "meaning": "No router port forwarding targets the TOR home server.",
        },
        {
            "id": "local_bindings",
            "ok": bindings == EXPECTED_HOME_BINDINGS,
            "meaning": "Proposed application, database and internal proxy bind only locally.",
        },
    ]
    passed = all(v["ok"] for v in results)
    return {
        "schema": "tor.home_ingress_preflight_result.v1",
        "overall": "PASS" if passed else "BLOCKED",
        "scope": "operator-supplied snapshot only, no live inspection",
        "checks": results,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path,
                        help="Local sanitized snapshot; never use a secrets file.")
    parser.add_argument("--output", type=Path,
                        help="Optional local JSON report with no raw IPs or keys.")
    args = parser.parse_args()
    try:
        if args.input.stat().st_size > 65536:
            raise SnapshotError("Snapshot exceeds 64 KiB.")
        with args.input.open(encoding="utf-8") as stream:
            result = check(json.load(stream))
    except (SnapshotError, ValueError, OSError, json.JSONDecodeError):
        print("TOR_NETWORK_PREFLIGHT_BLOCKED: malformed or inaccessible snapshot.",
              file=sys.stderr)
        return 2

    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        try:
            args.output.write_text(rendered, encoding="utf-8")
        except OSError:
            print("TOR_NETWORK_PREFLIGHT_BLOCKED: cannot save report.",
                  file=sys.stderr)
            return 2
    else:
        sys.stdout.write(rendered)
    return 0 if result["overall"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
