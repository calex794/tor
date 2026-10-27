#!/usr/bin/env python3
"""Conservative I-07/S062 selection for existing TOR CI workflows, Python 3.11+."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tomllib

from ci_test_map import (
    CRITICAL_PACKAGES, KNOWN_PACKAGES, PROSE_FILES, PROSE_PREFIXES,
    SUITES, TEST_SUITES,
)


def cargo_graph(root: Path):
    """Read local dependencies, including aliases, optional, dev/build and targets."""
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    manifests = {}
    directories = {}
    for member in workspace["members"]:
        if any(c in member for c in "*?["):
            raise ValueError("unreviewed workspace glob")
        directory = (root / member).resolve()
        if not directory.is_relative_to(root.resolve()):
            raise ValueError("workspace member outside root")
        manifest = tomllib.loads((directory / "Cargo.toml").read_text())
        name = manifest["package"]["name"]
        if name not in KNOWN_PACKAGES or name in manifests:
            raise ValueError("unreviewed or duplicate Cargo package")
        manifests[name] = manifest
        directories[directory] = name
    graph = {name: set() for name in manifests}
    inherited = workspace.get("dependencies", {})
    for directory, name in directories.items():
        manifest = manifests[name]
        scopes = [manifest, *manifest.get("target", {}).values()]
        for scope in scopes:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for alias, dep in scope.get(kind, {}).items():
                    if not isinstance(dep, dict):
                        continue
                    base = directory
                    if dep.get("workspace"):
                        dep = inherited[alias]
                        base = root.resolve()
                    if isinstance(dep, dict) and "path" in dep:
                        target = (base / dep["path"]).resolve()
                        if target not in directories:
                            raise ValueError("local dependency outside workspace")
                        graph[name].add(directories[target])
    if not graph:
        raise ValueError("empty Cargo workspace")
    return graph, {p.relative_to(root.resolve()).as_posix(): n for p, n in directories.items()}


def closure(graph, roots):
    pending = list(graph if "*" in roots else roots)
    visited = set()
    while pending:
        name = pending.pop()
        if name not in graph:
            raise ValueError("CI root missing from Cargo workspace")
        if name not in visited:
            visited.add(name)
            pending.extend(graph[name] - visited)
    return visited


def report(paths, selected, reasons, packages=(), full=False):
    suites = {name: full or name in selected for name in SUITES}
    return {
        "schema_version": 1,
        "mode": "full" if full else "feature" if any(suites.values()) else "cheap",
        "groups": {
            "cheap": True, "feature": suites["rust"], "postgres": suites["postgres"],
            "browser": any(suites[s] for s in ("reader_browser", "visual", "cover_browser")),
            "full": full,
        },
        "suites": suites, "packages": sorted(packages),
        "changed_paths": sorted(set(paths)), "reasons": sorted(set(reasons)),
    }


def select(root: Path, paths, force_full=None):
    paths = sorted(set(paths))
    if force_full or not paths:
        return report(paths, (), [force_full or "empty diff: full safety pack"], full=True)
    try:
        graph, directories = cargo_graph(root)
        consumed = {name: closure(graph, roots) for name, (_, roots) in SUITES.items()}
    except (OSError, ValueError, KeyError, TypeError) as exc:
        return report(paths, (), [f"unreadable dependency graph: {type(exc).__name__}"], full=True)
    selected, packages, reasons = set(), set(), []
    for path in paths:
        parts = PurePosixPath(path).parts
        if not parts or path.startswith("/") or ".." in parts or "\\" in path:
            return report(paths, (), ["non-canonical changed path"], full=True)
        if path in TEST_SUITES:
            selected.update(TEST_SUITES[path])
            reasons.append(f"test harness: {path}")
            continue
        if path in PROSE_FILES or (path.endswith(".md") and path.startswith(PROSE_PREFIXES)):
            reasons.append(f"prose: {path}")
            continue
        directory = "/".join(parts[:2])
        package = directories.get(directory)
        if not package or len(parts) < 4 or parts[2] != "src":
            return report(paths, (), [f"shared or unclassified input: {path}"], full=True)
        if package in CRITICAL_PACKAGES:
            return report(paths, (), [f"access/money/delivery/schema: {path}"], full=True)
        if package == "tor-web":
            return report(paths, (), [f"HTTP wiring or security boundary: {path}"], full=True)
        # Library changes run all suites compiling that package, transitively.
        selected.update(name for name, deps in consumed.items() if package in deps)
        packages.update(name for name in graph if package in closure(graph, (name,)))
        reasons.append(f"Cargo consumers of {package}")
    return report(paths, selected, reasons, packages)


def changed_paths(root: Path, base: str, head: str):
    # Ref values come from event JSON, never from interpolated shell expressions.
    if not all(re.fullmatch(r"[0-9a-f]{40}", ref) for ref in (base, head)):
        raise ValueError("expected two complete commit SHAs")
    if base == "0" * 40 or head == "0" * 40:
        raise ValueError("missing history")
    output = subprocess.run(
        ["git", "diff", "--no-ext-diff", "--no-renames", "--name-only", "-z",
         f"{base}...{head}", "--"],
        cwd=root, check=True, capture_output=True, timeout=30,
    ).stdout
    # No rename optimisation: both deleted old and added new paths affect safety.
    return [p.decode("utf-8") for p in output.split(b"\0") if p]


def event_selection(root: Path, event_name: str, event: dict, head: str):
    # Every integrated main SHA and manual run receives all available suites.
    if event_name != "pull_request":
        result = select(root, [], f"{event_name or 'unknown event'}: full integration pack")
        result.update(base_sha=None, head_sha=head)
        return result
    base = None
    try:
        base = event["pull_request"]["base"]["sha"]
        paths = changed_paths(root, base, head)
        result = select(root, paths)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as exc:
        result = select(root, [], f"diff unavailable: {type(exc).__name__}")
    result.update(base_sha=base, head_sha=head)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--path", action="append", help="explicit paths for local inspection")
    parser.add_argument("--base")
    parser.add_argument("--head", default=os.environ.get("GITHUB_SHA", ""))
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    if args.full:
        result = select(args.root, [], "explicit full pack")
    elif args.path is not None:
        result = select(args.root, args.path)
    elif args.base:
        result = event_selection(args.root, "pull_request", {
            "pull_request": {"base": {"sha": args.base}}
        }, args.head)
    else:
        try:
            event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
            result = event_selection(args.root, os.environ.get("GITHUB_EVENT_NAME", ""), event, args.head)
        except (OSError, ValueError, KeyError, TypeError) as exc:
            result = select(args.root, [], f"event unavailable: {type(exc).__name__}")
    serialized = json.dumps(result, ensure_ascii=True, indent=2) + "\n"
    print(serialized, end="")
    if args.report:
        args.report.write_text(serialized)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as stream:
            stream.write("suites=" + json.dumps(result["suites"], separators=(",", ":")) + "\n")
            stream.write("mode=" + result["mode"] + "\n")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a", encoding="utf-8") as stream:
            enabled = ", ".join(k for k, v in result["suites"].items() if v) or "repository rules only"
            stream.write(f"CI selection: **{result['mode']}**. Suites: {enabled}.\n")
            stream.write("Exact changed paths, reasons and SHAs are in the selection artifact.\n")


if __name__ == "__main__":
    main()
