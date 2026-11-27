"""ADAPT-00: lexical candidate inventory, never an authorization to alter a limit."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import subprocess

EXTENSIONS = frozenset({
    ".rs", ".sql", ".py", ".sh", ".js", ".cjs", ".mjs", ".ts", ".tsx",
    ".toml", ".yaml", ".yml", ".json", ".html", ".css", ".conf",
    ".service", ".timer", ".example",
})
KEYWORDS = r"(?:LIMIT|MAX|MIN|CAPACITY|BUDGET|BATCH|QUOTA|POOL|TIMEOUT|DEADLINE|INTERVAL|CONCURRENCY|PARALLEL|WINDOW|RETRY|BACKOFF|LEASE|TTL|SIZE|BYTES|PERMITS|RATE|WORKERS|THROTTLE|PAGE|PREFETCH|CACHE)"
RULES = (
    ("named_constant", re.compile(r"\bconst\s+(?P<symbol>[A-Z][A-Z0-9_]*" + KEYWORDS + r"[A-Z0-9_]*|"+KEYWORDS+r"[A-Z0-9_]*)\s*:")),
    ("environment_key", re.compile(r"\b(?P<symbol>TOR_[A-Z0-9_]*"+KEYWORDS+r"[A-Z0-9_]*)\b")),
    ("duration", re.compile(r"\bDuration::from_(?:secs|millis|micros|nanos)(?:_f64)?\s*\(")),
    ("capacity_call", re.compile(r"\b(?:Semaphore::new|(?:max|min)_connections|acquire_timeout|idle_timeout|buffer_unordered|for_each_concurrent|DefaultBodyLimit::max|with_capacity|chunks|chunked|truncate|take)\s*\(")),
    ("numeric_range", re.compile(r"\b[0-9][0-9_]*\s*\.\.=?\s*[0-9][0-9_]*\b")),
    ("size_guard", re.compile(r"\b(?:len|size_bytes|content_length|pixel_count)\s*(?:\(\))?\s*(?:>=|<=|>|<)\s*")),
    ("sql_limit", re.compile(r"\bLIMIT\s+(?:\$\d+|[0-9]+|[A-Z_][A-Z0-9_]*|\()", re.IGNORECASE)),
    ("throttle_call", re.compile(r"\b(?:permit_auth_attempt|permit_scoped_action|rate_limit|rate_limit_ip)\s*\(")),
    ("operator_config", re.compile(r"^\s*(?P<symbol>MemoryMax|MemoryHigh|TasksMax|CPUQuota|LimitNOFILE|RestartSec|TimeoutStartSec|TimeoutStopSec|worker_connections|worker_processes|limit_req(?:_zone)?|limit_conn(?:_zone)?|client_max_body_size|proxy_read_timeout|proxy_connect_timeout|keepalive_timeout|max_connections|shared_buffers|work_mem|statement_timeout|lock_timeout|idle_in_transaction_session_timeout|timeout-minutes|max-parallel)\b", re.IGNORECASE)),
)


def scope(path):
    parts = Path(path).parts
    if "tests" in parts or "fixtures" in parts or path.endswith(("_test.rs", "_tests.rs", ".test.cjs", ".test.mjs")):
        return "test"
    if parts[0] in ("ops", "deploy", "deployment", "infra", ".github", "config", "scripts"):
        return "operator_or_ci"
    if parts[0] in ("tools", "docs", "architecture") or "examples" in parts:
        return "tool_or_reference"
    return "source"


def scan_text(path, content):
    """Report only coordinates and known safe identifier matches, not source lines."""
    result = []
    for lineno, line in enumerate(content.splitlines(), 1):
        for kind, pattern in RULES:
            for match in pattern.finditer(line):
                item = {
                    "path": path, "line": lineno, "column": match.start() + 1,
                    "rule": kind, "scope": scope(path), "classification": "unreviewed",
                }
                if "symbol" in match.groupdict():
                    item["symbol"] = match.group("symbol")
                result.append(item)
    return result


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args])


def inventory(root):
    root = Path(root).resolve()
    sha = git(root, "rev-parse", "HEAD").decode().strip()
    dirty = bool(git(root, "status", "--porcelain", "--untracked-files=no"))
    paths = sorted(set(git(root, "ls-files", "-z", "--cached").decode("utf-8").split("\0")) - {""})
    skipped = Counter()
    scanned = Counter()
    candidates = []
    for name in paths:
        path = root / name
        if path.suffix.lower() not in EXTENSIONS:
            skipped["unsupported_extension"] += 1
            continue
        if path.is_symlink() or not path.resolve().is_relative_to(root):
            skipped["symlink_or_external"] += 1
            continue
        # Missing or invalid UTF-8 source is an explicit failure, not a fake clean scan.
        content = path.read_text(encoding="utf-8")
        scanned[path.suffix.lower()] += 1
        candidates.extend(scan_text(name, content))
    return {
        "schema_version": 1, "evidence": "LEXICAL_CANDIDATES_NOT_SEMANTIC_AUDIT",
        "commit": sha, "tracked_worktree_dirty": dirty,
        "tracked_files": len(paths), "scanned_files": sum(scanned.values()),
        "scanned_by_extension": dict(sorted(scanned.items())),
        "skipped": dict(sorted(skipped.items())),
        "candidate_count": len(candidates),
        "by_rule": dict(sorted(Counter(c["rule"] for c in candidates).items())),
        "by_scope": dict(sorted(Counter(c["scope"] for c in candidates).items())),
        "candidates": candidates,
    }


def summary(report):
    return {key: value for key, value in report.items() if key != "candidates"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path, help="Write full JSON candidate inventory")
    args = parser.parse_args()
    report = inventory(args.root)
    if args.output is not None:
        args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(summary(report), sort_keys=True))


if __name__ == "__main__":
    main()
