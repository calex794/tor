#!/usr/bin/env python3
"""Enforce TOR's feature-owned layers and conservative Cargo boundaries."""
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LAYERS = {"api", "services", "repositories", "schemas", "domain"}
SPECIAL = {"contracts", "web"}
DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


def _toml(path: Path) -> dict:
    with path.open("rb") as stream:
        return tomllib.load(stream)


def _dependencies(package: dict):
    scopes = [package]
    scopes.extend(v for v in package.get("target", {}).values() if isinstance(v, dict))
    for scope in scopes:
        for section in DEPENDENCY_SECTIONS:
            for alias, spec in scope.get(section, {}).items():
                yield alias, spec


def check(root: Path = ROOT) -> list[str]:
    errors: list[str] = []
    if (root / "src").exists():
        errors.append("root src/ is forbidden: use crates/<feature>/src or crates/web/src")
    if (root / "services").exists() or (root / "repositories").exists():
        errors.append("flat services/ or repositories/ is forbidden")
    cargo = root / "Cargo.toml"
    crates = root / "crates"
    if not cargo.is_file():
        if crates.exists() and any(crates.iterdir()):
            errors.append("crates/ exists but root Cargo.toml is missing")
        return errors  # TOR is being initialized, not yet a Rust workspace.
    try:
        workspace = _toml(cargo).get("workspace", {})
        members = workspace.get("members", [])
        if not isinstance(members, list) or any(not isinstance(x, str) for x in members):
            return errors + ["workspace.members must be a list of explicit crate paths"]
        if any("*" in x for x in members):
            errors.append("workspace.members must list crates explicitly, without globs")
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        return errors + [f"invalid root Cargo.toml: {exc}"]
    owner_file = root / "architecture" / "modules.toml"
    if not owner_file.is_file():
        return errors + ["missing architecture/modules.toml feature ownership registry"]
    try:
        registered = _toml(owner_file).get("features", {})
        if not isinstance(registered, dict):
            return errors + ["architecture/modules.toml needs a [features] table"]
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return errors + [f"invalid feature ownership registry: {exc}"]
    if not crates.is_dir():
        return errors + ["missing crates/ directory"]
    dirs = sorted(p for p in crates.iterdir() if p.is_dir())
    found = {f"crates/{p.name}" for p in dirs}
    declared = set(members)
    if found != declared:
        errors.append(f"workspace mismatch; unlisted={sorted(found-declared)}, stale={sorted(declared-found)}")
    workspace_deps = workspace.get("dependencies", {})
    for folder in dirs:
        name = folder.name
        manifest = folder / "Cargo.toml"
        if not manifest.is_file():
            errors.append(f"missing Cargo.toml in crates/{name}")
            continue
        if name not in SPECIAL and name not in registered:
            errors.append(f"unregistered feature crates/{name}; add a named owner to architecture/modules.toml")
        try:
            data = _toml(manifest)
        except (OSError, tomllib.TOMLDecodeError) as exc:
            errors.append(f"invalid {manifest.relative_to(root)}: {exc}")
            continue
        package_name = data.get("package", {}).get("name")
        if package_name != f"tor-{name}":
            errors.append(f"crates/{name} package must be named tor-{name}, not {package_name}")
        src = folder / "src"
        if not src.is_dir():
            errors.append(f"missing crates/{name}/src")
        elif name not in SPECIAL:
            for child in src.iterdir():
                if child.is_dir() and child.name not in LAYERS:
                    errors.append(f"unexpected feature directory: crates/{name}/src/{child.name}")
                if child.is_file() and child.suffix == ".rs" and child.name != "lib.rs":
                    errors.append(f"flat feature module forbidden: crates/{name}/src/{child.name}")
            if not any((src / layer).is_dir() for layer in LAYERS):
                errors.append(f"feature crates/{name} must own at least one layer folder")
            test_files = list((folder / "tests").rglob("*.rs")) if (folder / "tests").exists() else []
            inline_tests = any("#[cfg(test)]" in p.read_text(encoding="utf8")
                               for p in src.rglob("*.rs"))
            if not test_files and not inline_tests:
                errors.append(f"feature crates/{name} has no colocated tests")
        for alias, raw in _dependencies(data):
            spec = workspace_deps.get(alias, {}) if isinstance(raw, dict) and raw.get("workspace") else raw
            if not isinstance(spec, dict):
                spec = {}
            target = None
            if "path" in spec:
                base = root if isinstance(raw, dict) and raw.get("workspace") else folder
                dep_dir = (base / spec["path"]).resolve()
                if dep_dir.parent == crates.resolve():
                    target = dep_dir.name
            if target is None:
                pkg = spec.get("package", alias)
                if isinstance(pkg, str) and pkg.startswith("tor-"):
                    target = pkg.removeprefix("tor-")
            if target is None:
                continue
            if target not in SPECIAL and target not in registered:
                errors.append(f"crates/{name}: unknown local dependency {alias} -> {target}")
            elif name == "contracts":
                errors.append(f"contracts cannot depend on {target}")
            elif name != "web" and target not in {"contracts", name}:
                errors.append(f"cross-feature dependency: crates/{name} -> crates/{target}")
        if name in SPECIAL:
            forbidden = {"sqlx"} if name == "web" else {"axum", "sqlx"}
            keys = {alias for alias, _ in _dependencies(data)}
            for item in sorted(keys & forbidden):
                errors.append(f"crates/{name} must not depend on {item}")
        for source in folder.rglob("*.rs"):
            relative = source.relative_to(root).as_posix()
            parts = source.relative_to(folder).parts
            is_test = "tests" in parts
            lines = source.read_text(encoding="utf8").splitlines()
            max_lines = 1000 if is_test else 600
            warn_lines = 800 if is_test else 400
            if len(lines) > max_lines:
                errors.append(f"source too large: {relative} ({len(lines)} > {max_lines})")
            elif len(lines) > warn_lines:
                print(f"WARN: {relative}: {len(lines)} lines", file=sys.stderr)
            if len(parts) < 3 or parts[0] != "src":
                continue
            layer = parts[1]
            source_text = "\n".join(lines)
            if layer in {"api", "services", "schemas", "domain"} and re.search(r"\bsqlx\s*::", source_text):
                errors.append(f"SQLx outside repositories: {relative}")
            if layer == "domain" and re.search(r"\baxum\s*::", source_text):
                errors.append(f"Axum inside domain: {relative}")
    return errors


if __name__ == "__main__":
    problems = check()
    for issue in problems:
        print("FAIL:", issue, file=sys.stderr)
    if problems:
        raise SystemExit(1)
    print("TOR feature architecture: PASS")
