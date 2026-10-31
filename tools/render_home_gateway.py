#!/usr/bin/env python3
"""Render reviewed, full Nginx configurations without shell substitution.

Only fixed template placeholders are expanded; Nginx $variables are preserved.
No provisioning, SSH, secret generation or service restart occurs.
"""
from __future__ import annotations

import argparse
import ipaddress
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEMPLATES = ROOT / "ops" / "home-gateway"
SAFE_PATH = re.compile(r"/[A-Za-z0-9_./-]+$")
SAFE_DOMAIN = re.compile(
    r"(?=.{1,253}$)(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+"
    r"[A-Za-z]{2,63}$"
)


def endpoint(value: str) -> str:
    host, sep, port = value.rpartition(":")
    if not sep or not port.isascii() or not port.isdecimal():
        raise ValueError("endpoint must be an IPv4 address and TCP port")
    ipaddress.IPv4Address(host)
    number = int(port)
    if not 1 <= number <= 65535:
        raise ValueError("port must be 1..65535")
    return f"{host}:{number}"


def absolute_path(value: str) -> str:
    if not SAFE_PATH.fullmatch(value) or "/../" in value or value.endswith("/.."):
        raise ValueError("paths must be safe absolute POSIX paths")
    return value


def render(args: argparse.Namespace) -> str:
    substitutions = {
        "@PID@": absolute_path(args.pid),
        "@LISTEN@": endpoint(args.listen),
    }
    if args.role == "edge":
        if not args.home:
            raise ValueError("edge requires --home")
        template = "edge.nginx.conf.in"
        substitutions["@HOME_ENDPOINT@"] = endpoint(args.home)
    else:
        if not all((args.edge_ip, args.backend, args.domain, args.cert, args.key)):
            raise ValueError("home requires --edge-ip, --backend, --domain, --cert and --key")
        if not SAFE_DOMAIN.fullmatch(args.domain):
            raise ValueError("invalid TLS server name")
        backend = endpoint(args.backend)
        if not backend.startswith("127."):
            raise ValueError("the Rust backend must listen on loopback only")
        template = "home.nginx.conf.in"
        substitutions.update({
            "@EDGE_IP@": str(ipaddress.IPv4Address(args.edge_ip)),
            "@BACKEND@": backend,
            "@DOMAIN@": args.domain,
            "@CERT@": absolute_path(args.cert),
            "@KEY@": absolute_path(args.key),
        })
    value = (TEMPLATES / template).read_text(encoding="utf-8")
    for tag, replacement in substitutions.items():
        value = value.replace(tag, replacement)
    if re.search(r"@[A-Z_]+@", value):
        raise ValueError("unrendered template token")
    return value


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("role", choices=("edge", "home"))
    parser.add_argument("--listen", required=True, help="IPv4:port on this host")
    parser.add_argument("--home", help="IPv4:port of the home WireGuard listener")
    parser.add_argument("--edge-ip", help="trusted WireGuard peer IPv4 (home role)")
    parser.add_argument("--backend", help="local 127.x.y.z:port of TOR (home role)")
    parser.add_argument("--domain", help="HTTPS domain (home role)")
    parser.add_argument("--cert", help="absolute path to the certificate (home role)")
    parser.add_argument("--key", help="absolute path to the private key (home role)")
    parser.add_argument("--pid", required=True, help="absolute Nginx master PID file")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = render(args)
    except ValueError as error:
        parser.error(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(result, encoding="utf-8")


if __name__ == "__main__":
    main()
