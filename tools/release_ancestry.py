#!/usr/bin/env python3
# TOR: shared immutable forward-release ancestry guard.
"""Fail closed unless a pinned release is on main and not behind active code."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from urllib.parse import urlencode
from urllib.request import Request, urlopen

REPO = "calex794/tor"
SHA = re.compile(r"[a-f0-9]{40}\Z")


class AncestryError(ValueError):
    """Missing or unsafe Git ancestry evidence."""


def valid_sha(value):
    return isinstance(value, str) and SHA.fullmatch(value) is not None


def require_ancestor(get, base, descendant):
    """Prove equality or ancestry using immutable SHAs, never timestamps."""
    if not valid_sha(base) or not valid_sha(descendant):
        raise AncestryError("invalid ancestry SHA")
    if base == descendant:
        return
    proof = get(f"/repos/{REPO}/compare/{base}...{descendant}", {"per_page": 1})
    if not isinstance(proof, dict):
        raise AncestryError("malformed GitHub comparison")
    base_commit, merge_base = proof.get("base_commit"), proof.get("merge_base_commit")
    if (not isinstance(base_commit, dict) or base_commit.get("sha") != base
            or not isinstance(merge_base, dict) or merge_base.get("sha") != base
            or proof.get("status") != "ahead"
            or type(proof.get("behind_by")) is not int or proof["behind_by"] != 0
            or type(proof.get("ahead_by")) is not int or proof["ahead_by"] <= 0):
        raise AncestryError("candidate is not a verified forward descendant")


def require_on_main(get, candidate):
    if not valid_sha(candidate):
        raise AncestryError("invalid candidate SHA")
    ref = get(f"/repos/{REPO}/git/ref/heads/main")
    if not isinstance(ref, dict):
        raise AncestryError("malformed main ref")
    obj = ref.get("object")
    if (ref.get("ref") != "refs/heads/main" or not isinstance(obj, dict)
            or obj.get("type") != "commit" or not valid_sha(obj.get("sha"))):
        raise AncestryError("invalid main ref")
    require_ancestor(get, candidate, obj["sha"])
    return obj["sha"]


def verify_forward(get, candidate, active=None):
    main = require_on_main(get, candidate)
    if active is not None:
        require_ancestor(get, active, candidate)
    return main


def public_get(path, params=None):
    url = "https://api.github.com" + path
    if params:
        url += "?" + urlencode(params)
    request = Request(url, headers={
        "Accept": "application/vnd.github+json",
        "Cache-Control": "no-cache",
        "User-Agent": "tor-release-ancestry",
    })
    with urlopen(request, timeout=20) as response:
        return json.load(response)


def active_sha():
    current = Path("/opt/tor/current")
    if not current.is_symlink():
        raise AncestryError("active release is not a symlink")
    active = current.resolve(strict=True)
    if (active.parent != Path("/opt/tor/releases") or not valid_sha(active.name)
            or not active.is_dir() or not (active / "tor-web").is_file()):
        raise AncestryError("active release is outside the controlled SHA directories")
    return active.name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sha")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--main-only", action="store_true")
    mode.add_argument("--from-active", action="store_true")
    args = parser.parse_args()
    try:
        active = active_sha() if args.from_active else None
        head = verify_forward(public_get, args.sha, active)
    except (ValueError, OSError, RuntimeError) as exc:
        print(f"TOR_RELEASE_REFUSED: ancestry proof failed: {type(exc).__name__}",
              file=sys.stderr)
        return 1
    print(f"TOR_RELEASE_ANCESTRY_OK active={active or 'not-checked'} "
          f"candidate={args.sha} main={head}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
