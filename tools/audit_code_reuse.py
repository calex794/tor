#!/usr/bin/env python3
"""REUSE-01: deterministic, read-only clone candidates from a committed Git tree."""
import argparse
from collections import defaultdict
from dataclasses import dataclass
import io
from itertools import combinations
import json
from pathlib import Path, PurePosixPath
import re
import subprocess

EXTENSIONS = {".rs", ".js", ".cjs", ".mjs", ".css", ".html", ".py",
              ".sql", ".toml", ".yml", ".yaml", ".sh"}
SCHEMA = "tor.reuse.audit.v1"


def git(root, *args, input_bytes=None):
    return subprocess.run(
        ["git", "-C", str(root), *args], input=input_bytes,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
    ).stdout


@dataclass(frozen=True)
class Source:
    path: str
    oid: str
    lines: tuple[str, ...]
    physical: tuple[int, ...]

    @classmethod
    def from_bytes(cls, path, oid, raw):
        lines = []
        physical = []
        for number, line in enumerate(raw.decode("utf-8").splitlines(), 1):
            if line.strip():
                lines.append(line.strip())
                physical.append(number)
        return cls(path, oid, tuple(lines), tuple(physical))

    @property
    def language(self):
        ext = PurePosixPath(self.path).suffix
        return ".js" if ext in {".cjs", ".mjs"} else ext

    @property
    def area(self):
        parts = PurePosixPath(self.path).parts
        return "/".join(parts[:2]) if parts[0] == "crates" else parts[0]

    @property
    def scope(self):
        p = PurePosixPath(self.path)
        if ("tests" in p.parts or "fixtures" in p.parts
                or re.search(r"(^test_|_tests?\.|\.test\.)", p.name)
                or p.name in {"tests.rs", "test.rs"}):
            return "test"
        if p.parts[0] == "crates":
            return "application"
        return "infrastructure"


def read_tree(root, ref):
    commit = git(root, "rev-parse", "--verify", "--end-of-options",
                 ref + "^{commit}").decode("ascii").strip()
    entries = []
    excluded = []
    for entry in git(root, "ls-tree", "-r", "-z", commit).split(b"\0"):
        if not entry:
            continue
        meta, name = entry.split(b"\t", 1)
        mode, kind, oid = meta.decode("ascii").split()
        path = name.decode("utf-8", errors="surrogateescape")
        if PurePosixPath(path).suffix not in EXTENSIONS:
            continue
        if kind != "blob" or mode not in {"100644", "100755"}:
            excluded.append({"path": path, "reason": "not_regular_blob"})
            continue
        entries.append((path, oid))
    entries.sort()
    oids = sorted({oid for _, oid in entries})
    raw = io.BytesIO(git(root, "cat-file", "--batch",
                         input_bytes=("".join(o + "\n" for o in oids)).encode("ascii")))
    blobs = {}
    for oid in oids:
        header = raw.readline().decode("ascii").split()
        if len(header) != 3 or header[:2] != [oid, "blob"]:
            raise ValueError("unexpected git cat-file response")
        size = int(header[2])
        data = raw.read(size)
        if len(data) != size or raw.read(1) != b"\n":
            raise ValueError("incomplete git blob")
        blobs[oid] = data
    if raw.read(1):
        raise ValueError("unexpected trailing Git data")
    sources = []
    for path, oid in entries:
        try:
            if b"\0" in blobs[oid]:
                raise UnicodeError("binary content")
            sources.append(Source.from_bytes(path, oid, blobs[oid]))
        except UnicodeError:
            excluded.append({"path": path, "reason": "not_utf8_text"})
    return commit, sources, sorted(excluded, key=lambda e: e["path"])


def location(source, start, length):
    return {"path": source.path, "scope": source.scope,
            "start_line": source.physical[start],
            "end_line": source.physical[start + length - 1]}


def audit(sources, *, window=8, min_chars=160, occurrence_cap=16, top=100):
    """Compare same-language windows, merge adjacent hits; no semantic claims."""
    if not 4 <= window <= 64 or min_chars < 1 or not 2 <= occurrence_cap <= 32 or top < 0:
        raise ValueError("invalid audit limits")
    sources = sorted(sources, key=lambda s: s.path)
    exact = defaultdict(list)
    index = defaultdict(list)
    for file_id, source in enumerate(sources):
        exact[source.oid].append(source.path)
        for start in range(len(source.lines) - window + 1):
            chunk = source.lines[start:start + window]
            if sum(map(len, chunk)) < min_chars:
                continue
            key = (source.language, chunk)
            bucket = index[key]
            # Retain one overflow marker, not an unbounded list of boilerplate.
            if len(bucket) <= occurrence_cap:
                bucket.append((file_id, start))

    repeated = defaultdict(list)
    skipped = 0
    for hits in index.values():
        if len(hits) > occurrence_cap:
            skipped += 1
            continue
        for (a, left), (b, right) in combinations(hits, 2):
            if a == b and right - left < window:
                continue
            repeated[a, b, right - left].append(left)

    covered = [set() for _ in sources]
    candidates = []
    for (a, b, delta), positions in repeated.items():
        positions = sorted(set(positions))
        first = last = positions[0]
        for next_start in positions[1:] + [None]:
            if next_start == last + 1:
                last = next_start
                continue
            length = last - first + window
            # Never report two overlapping copies inside the same file.
            if a == b:
                length = min(length, delta)
            covered[a].update(range(first, first + length))
            covered[b].update(range(first + delta, first + delta + length))
            candidates.append({
                "nonblank_lines": length,
                "left": location(sources[a], first, length),
                "right": location(sources[b], first + delta, length),
            })
            first = last = next_start

    candidates.sort(key=lambda c: (-c["nonblank_lines"], c["left"]["path"],
                                   c["left"]["start_line"], c["right"]["path"],
                                   c["right"]["start_line"]))
    areas = defaultdict(lambda: {"files": 0, "nonblank_lines": 0,
                                 "candidate_lines": 0})
    scopes = defaultdict(lambda: {"files": 0, "nonblank_lines": 0,
                                  "candidate_lines": 0})
    for i, source in enumerate(sources):
        for stats in (areas[source.area], scopes[source.scope]):
            stats["files"] += 1
            stats["nonblank_lines"] += len(source.lines)
            stats["candidate_lines"] += len(covered[i])
    exact_groups = sorted(sorted(paths) for paths in exact.values() if len(paths) > 1)
    return {
        "schema": SCHEMA,
        "settings": {"window": window, "min_chars": min_chars,
                     "occurrence_cap": occurrence_cap, "top": top},
        "files": len(sources),
        "nonblank_lines": sum(len(s.lines) for s in sources),
        "candidate_lines": sum(map(len, covered)),
        "candidate_pairs": len(candidates),
        "high_frequency_windows_skipped": skipped,
        "byte_identical_file_groups": exact_groups,
        "by_area": dict(sorted(areas.items())),
        "by_scope": dict(sorted(scopes.items())),
        "candidates": candidates[:top],
    }


def audit_git(root, ref="HEAD", **settings):
    commit, sources, excluded = read_tree(root, ref)
    return {"commit": commit, "excluded": excluded, **audit(sources, **settings)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--ref", default="HEAD")
    parser.add_argument("--window", type=int, default=8)
    parser.add_argument("--min-chars", type=int, default=160)
    parser.add_argument("--occurrence-cap", type=int, default=16)
    parser.add_argument("--top", type=int, default=100)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = audit_git(args.root, args.ref, window=args.window,
                       min_chars=args.min_chars, occurrence_cap=args.occurrence_cap,
                       top=args.top)
    report = json.dumps(result, ensure_ascii=True, sort_keys=True, indent=2) + "\n"
    if args.output:
        args.output.write_text(report, encoding="utf-8")
    else:
        print(report, end="")


if __name__ == "__main__":
    main()
