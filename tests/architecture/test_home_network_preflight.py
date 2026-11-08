"""Offline network-preflight regression tests: no sockets or system inspection."""
import copy
import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools"))
from home_network_preflight import check, SnapshotError

SAFE = {
    "schema": "tor.home_ingress_preflight.v1",
    "tunnel_network": "10.88.0.0/30",
    "edge_tunnel_ip": "10.88.0.1",
    "home_tunnel_ip": "10.88.0.2",
    "other_networks": [
        {"role": "home_lan", "cidr": "192.168.1.0/24"},
        {"role": "vpn", "cidr": "10.30.0.0/24"},
    ],
    "edge_wg_udp_port": 51820,
    "edge_busy_udp_ports": [39743],
    "edge_tcp_service_owners": {"80": "nginx", "443": "nginx"},
    "edge_peer_allowed_ips": ["10.88.0.2/32"],
    "home_peer_allowed_ips": ["10.88.0.1/32"],
    "home_initiates_tunnel": True,
    "home_default_route_through_tunnel": False,
    "home_router_to_tor_port_forwards": [],
    "home_service_bind_candidates": {
        "tor-web": ["127.0.0.1:3000"],
        "postgresql": ["127.0.0.1:5432"],
        "home-nginx": ["10.88.0.2:18081"],
    },
}


class PreflightTests(unittest.TestCase):
    def test_valid_snapshot_is_only_an_offline_review(self):
        result = check(copy.deepcopy(SAFE))
        self.assertEqual(result["overall"], "PASS")
        self.assertEqual(len(result["checks"]), 8)
        self.assertIn("no live inspection", result["scope"])
        self.assertNotIn("192.168.1", str(result))

    def test_unsafe_networks_and_services_block(self):
        cases = [
            ("other_networks", [{"role": "vpn", "cidr": "10.88.0.0/24"}]),
            ("edge_busy_udp_ports", [51820]),
            ("edge_tcp_service_owners", {"80": "tor", "443": "nginx"}),
            ("edge_peer_allowed_ips", ["0.0.0.0/0"]),
            ("home_peer_allowed_ips", ["10.88.0.0/30"]),
            ("home_initiates_tunnel", False),
            ("home_default_route_through_tunnel", True),
            ("home_router_to_tor_port_forwards", [3000]),
            ("home_service_bind_candidates", {
                **SAFE["home_service_bind_candidates"],
                "postgresql": ["0.0.0.0:5432"],
            }),
            ("home_tunnel_ip", "10.88.0.3"),
            ("edge_wg_udp_port", 39743),
        ]
        for key, value in cases:
            with self.subTest(key=key, value=value):
                data = copy.deepcopy(SAFE)
                data[key] = value
                self.assertEqual(check(data)["overall"], "BLOCKED")

    def test_malformed_snapshot_fails_closed(self):
        cases = [
            {**SAFE, "unknown": "oops"},
            {k: v for k, v in SAFE.items() if k != "home_initiates_tunnel"},
            {**SAFE, "tunnel_network": "10.88.0.1/30"},
            {**SAFE, "home_initiates_tunnel": "yes"},
            {**SAFE, "edge_busy_udp_ports": [True]},
            {**SAFE, "other_networks": [{"role": "unknown", "cidr": "10.0.0.0/8"}]},
        ]
        for case in cases:
            with self.subTest(case=str(case)[:90]):
                with self.assertRaises(SnapshotError):
                    check(case)


if __name__ == "__main__":
    unittest.main()
