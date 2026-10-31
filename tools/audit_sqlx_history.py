#!/usr/bin/env python3
"""Read-only SQLx release audit: installed checksums and open-PR version clashes.

No code path executes a migration, mutates the database, or writes a GitHub ref.
The output is evidence for an operator decision, never deployment approval.
"""
import argparse
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import urllib.error
import urllib.request
from collections import defaultdict

MIGRATION = re.compile(r"^(?P<version>[0-9]{12})_(?P<slug>[a-z0-9_]+)\.sql$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
HEX_SHA384 = re.compile(r"^[a-fA-F0-9]{96}$")
COLUMNS = {"database_name", "version", "description", "checksum_sha384_hex", "success"}
COPY_SQL = (
    "COPY (SELECT current_database() AS database_name, version, description, "
    "encode(checksum, 'hex') AS checksum_sha384_hex, success "
    "FROM _sqlx_migrations ORDER BY version) TO STDOUT WITH (FORMAT csv, HEADER true)"
)


class AuditError(Exception):
    """Unsafe or incomplete evidence; fail closed rather than guessing."""


def migration_identity(path):
    name = Path(path).name
    match = MIGRATION.fullmatch(name)
    if not match:
        raise AuditError(f"Invalid migration filename: {name}")
    return match.group("version"), match.group("slug").replace("_", " ")


def local_migrations(directory):
    root = Path(directory)
    if not root.is_dir():
        raise AuditError("Migration directory does not exist")
    files = sorted(root.glob("*.sql"))
    if not files:
        raise AuditError("Migration directory is empty")
    found = {}
    seen_checksum_versions = {}
    for path in files:
        version, description = migration_identity(path)
        if version in found:
            raise AuditError(f"Duplicate local SQLx version {version}")
        sql = path.read_bytes()
        try:
            sql.decode("utf-8")
        except UnicodeDecodeError as error:
            raise AuditError(f"Migration {path.name} is not UTF-8") from error
        checksum = hashlib.sha384(sql).hexdigest()
        if checksum in seen_checksum_versions:
            raise AuditError(
                "Identical SQLx SQL bytes under different local versions: "
                f"{seen_checksum_versions[checksum]}, {version}"
            )
        seen_checksum_versions[checksum] = version
        found[version] = {
            "filename": path.name,
            "description": description,
            "checksum": checksum,
        }
    return found


def parse_inventory(text):
    reader = csv.DictReader(io.StringIO(text, newline=""))
    if reader.fieldnames is None or set(reader.fieldnames) != COLUMNS:
        raise AuditError("Inventory CSV must have the exact five named columns")
    rows = {}
    database = None
    for item in reader:
        if None in item or any(value is None for value in item.values()):
            raise AuditError("Malformed inventory CSV")
        name = item["database_name"]
        version = item["version"]
        digest = item["checksum_sha384_hex"]
        flag = item["success"].lower()
        if not name or not re.fullmatch(r"[0-9]+", version):
            raise AuditError("Invalid database name or SQLx version")
        if database is not None and name != database:
            raise AuditError("Inventory mixes databases; audit one database at a time")
        if version in rows:
            raise AuditError(f"Duplicate applied SQLx version {version}")
        if not HEX_SHA384.fullmatch(digest) or flag not in {"t", "f", "true", "false"}:
            raise AuditError(f"Invalid checksum or success flag at version {version}")
        if not item["description"]:
            raise AuditError(f"Missing SQLx description at version {version}")
        database = name
        rows[version] = {
            "description": item["description"],
            "checksum": digest.lower(),
            "success": flag in {"t", "true"},
        }
    if not rows:
        raise AuditError("Empty SQLx inventory is not evidence of a migrated database")
    return database, rows


def evaluate_history(local, database, applied):
    blocked = []
    matched = []
    pending = []
    applied_checksum_versions = defaultdict(list)
    for version, installed in sorted(applied.items()):
        applied_checksum_versions[installed["checksum"]].append(version)
        expected = local.get(version)
        if not installed["success"]:
            blocked.append(f"{version}: failed SQLx migration recorded")
        if expected is None:
            blocked.append(f"{version}: installed migration absent from local tree")
            continue
        if installed["description"] != expected["description"]:
            blocked.append(f"{version}: installed SQLx description differs from local tree")
        if installed["checksum"] != expected["checksum"]:
            blocked.append(f"{version}: installed SQLx SHA-384 checksum differs from local tree")
        if (installed["success"] and installed["description"] == expected["description"]
                and installed["checksum"] == expected["checksum"]):
            matched.append(version)
    for versions in applied_checksum_versions.values():
        if len(versions) > 1:
            blocked.append(
                "Identical applied SQLx checksum under different versions: "
                + ", ".join(versions)
            )
    for version in sorted(local):
        if version not in applied:
            pending.append(version)
    max_installed = max(applied)
    older_pending = [version for version in pending if version < max_installed]
    if older_pending:
        blocked.append(
            "Unapplied SQLx versions older than latest applied version: "
            + ", ".join(older_pending)
        )
    return {
        "database_name": database,
        "status": "BLOCK" if blocked else "NO_APPLIED_DRIFT_DETECTED",
        "matched": matched,
        "pending": pending,
        "out_of_order_pending": older_pending,
        "blockers": blocked,
        "notice": (
            "Read-only snapshot; not a deploy approval. Unapplied migration "
            "versions below the latest applied version BLOCK this audit and "
            "require migration-owner compatibility review on an isolated restore."
        ),
    }


def read_from_psql():
    # Never infer a database from the shell user name.
    if not (os.environ.get("PGSERVICE") or os.environ.get("PGDATABASE")):
        raise AuditError("Set PGSERVICE or PGDATABASE for an explicitly selected DB")
    env = os.environ.copy()
    # Enforce read-only transaction defaults independently of the login role.
    env["PGOPTIONS"] = (
        "-c default_transaction_read_only=on "
        "-c statement_timeout=10000 -c lock_timeout=1000"
    )
    try:
        result = subprocess.run(
            ["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1", "-c", COPY_SQL],
            check=False, capture_output=True, text=True, timeout=20, env=env,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AuditError("Read-only psql inventory could not execute") from error
    if result.returncode != 0:
        # Avoid printing libpq diagnostics that can reveal a private endpoint.
        raise AuditError(f"Read-only psql inventory failed (exit {result.returncode})")
    return result.stdout


def save_private_inventory_csv(destination, raw_csv):
    """Store the exact SELECT-only ledger in a private, new regular file.

    The caller is responsible for selecting an approved output directory.
    Never echo the raw rows or allow a pre-existing file to be overwritten.
    """
    path = Path(destination)
    if path.name in {"", ".", ".."}:
        raise AuditError("Choose a private CSV output filename")
    if not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY"):
        raise AuditError("Safe private file creation is unavailable")
    try:
        directory_fd = os.open(
            path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
        )
    except OSError as error:
        raise AuditError("Private output directory is unavailable or symlinked") from error
    try:
        info = os.fstat(directory_fd)
        if (not stat.S_ISDIR(info.st_mode)
                or info.st_uid != os.geteuid()
                or info.st_mode & 0o077):
            raise AuditError(
                "Output directory must be owned by the operator and mode 0700"
            )
        try:
            file_fd = os.open(
                path.name,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                0o600,
                dir_fd=directory_fd,
            )
        except OSError as error:
            raise AuditError(
                "Private CSV must be a new file, not an existing path or symlink"
            ) from error
        try:
            with os.fdopen(file_fd, "w", encoding="utf-8", newline="") as output:
                output.write(raw_csv)
                output.flush()
                os.fsync(output.fileno())
        except (OSError, UnicodeError) as error:
            os.unlink(path.name, dir_fd=directory_fd)
            raise AuditError("Failed to save complete private CSV") from error
    finally:
        os.close(directory_fd)


def github_json(path, token=None):
    request = urllib.request.Request(
        "https://api.github.com" + path,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "tor-sqlx-readonly-audit",
            "X-GitHub-Api-Version": "2022-11-28",
            **({"Authorization": "Bearer " + token} if token else {}),
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)
    except (urllib.error.URLError, ValueError, TimeoutError) as error:
        raise AuditError("GitHub read-only API unavailable or returned invalid JSON") from error


def github_pages(path, token=None):
    results = []
    for page in range(1, 11):
        chunk = github_json(f"{path}&page={page}", token)
        if not isinstance(chunk, list):
            raise AuditError("Expected a list from paginated GitHub API")
        results.extend(chunk)
        if len(chunk) < 100:
            return results
    raise AuditError("GitHub query exceeded ten pages; do not report partial results")


def scan_conflicts(main_files, pull_requests):
    variants = defaultdict(lambda: defaultdict(set))

    def add(owner, filename, blob_sha):
        if not filename.startswith("migrations/") or not filename.endswith(".sql"):
            return
        version, _ = migration_identity(filename)
        if not isinstance(blob_sha, str) or not re.fullmatch(r"[0-9a-f]{40}", blob_sha):
            raise AuditError(f"Missing Git blob SHA for {filename}")
        variants[version][(filename, blob_sha)].add(owner)

    for file in main_files:
        add("main", file["path"], file["sha"])
    for pr in pull_requests:
        owner = f"#{pr['number']}"
        for file in pr["files"]:
            if file["status"] != "removed":
                add(owner, file["filename"], file["sha"])
    collisions = []
    for version, options in sorted(variants.items()):
        if len(options) > 1:
            collisions.append({
                "version": version,
                "variants": [
                    {"path": path, "blob_sha": sha, "owners": sorted(owners)}
                    for (path, sha), owners in sorted(options.items())
                ],
            })
    return collisions



def scan_cross_version_identical_sql(main_files, pull_requests):
    """Report exact Git blob reuse across distinct SQLx migration versions.

    This detects byte-identical SQL content under different numeric versions,
    not semantically equivalent SQL with different bytes. It is diagnostic:
    a matching blob does not establish that a migration ran twice in any DB.
    """
    by_blob = defaultdict(lambda: defaultdict(lambda: defaultdict(set)))

    def add(owner, filename, blob_sha):
        if not filename.startswith("migrations/") or not filename.endswith(".sql"):
            return
        version, _ = migration_identity(filename)
        if not isinstance(blob_sha, str) or not re.fullmatch(r"[0-9a-f]{40}", blob_sha):
            raise AuditError(f"Missing Git blob SHA for {filename}")
        by_blob[blob_sha][version][filename].add(owner)

    for file in main_files:
        add("main", file["path"], file["sha"])
    for pr in pull_requests:
        owner = f"#{pr['number']}"
        for file in pr["files"]:
            if file["status"] != "removed":
                add(owner, file["filename"], file["sha"])

    reused = []
    for blob_sha, versions in sorted(by_blob.items()):
        if len(versions) < 2:
            continue
        reused.append({
            "blob_sha": blob_sha,
            "versions": [
                {
                    "version": version,
                    "files": [
                        {"path": filename, "owners": sorted(owners)}
                        for filename, owners in sorted(files.items())
                    ],
                }
                for version, files in sorted(versions.items())
            ],
        })
    return reused


def snapshot_open_prs(prs):
    """Validate the complete OPEN PR number -> (head SHA, base SHA) map.

    PR file-list GETs are mutable. Never report a mixed PR snapshot as
    accepted conflict evidence when the repo changes mid-scan.
    """
    if not isinstance(prs, list):
        raise AuditError("Open PR inventory is not a list")
    snapshot = {}
    for pr in prs:
        if not isinstance(pr, dict):
            raise AuditError("Malformed open PR inventory")
        number, head, base = pr.get("number"), pr.get("head"), pr.get("base")
        if isinstance(number, bool) or not isinstance(number, int) or number <= 0:
            raise AuditError("Invalid open PR number")
        if number in snapshot:
            raise AuditError("Duplicate open PR number in paginated snapshot")
        if not isinstance(head, dict) or not isinstance(base, dict):
            raise AuditError("Missing open PR head/base identity")
        shas = (head.get("sha"), base.get("sha"))
        if any(not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha)
               for sha in shas):
            raise AuditError("Invalid open PR head/base SHA")
        snapshot[number] = shas
    return snapshot


def checked_main_sha(root, token):
    sha = github_json(f"{root}/branches/main", token)["commit"]["sha"]
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise AuditError("Invalid main SHA in GitHub response")
    return sha


def scan_live(repo):
    if not REPOSITORY.fullmatch(repo):
        raise AuditError("Repository must be in owner/name format")
    token = os.environ.get("GITHUB_TOKEN")
    root = f"/repos/{repo}"
    main_sha = checked_main_sha(root, token)
    main_files = github_json(f"{root}/contents/migrations?ref={main_sha}", token)
    if not isinstance(main_files, list):
        raise AuditError("Could not enumerate main migrations")
    prs = github_pages(f"{root}/pulls?state=open&per_page=100", token)
    initial_prs = snapshot_open_prs(prs)
    observed = []
    for pr in prs:
        files = github_pages(f"{root}/pulls/{pr['number']}/files?per_page=100", token)
        observed.append({"number": pr["number"], "head": pr["head"]["sha"], "files": files})
    # The file-list endpoint is not SHA-pinned. Fail closed if current main,
    # PR membership, heads or bases moved during the paginated file reads.
    if checked_main_sha(root, token) != main_sha:
        raise AuditError("main moved during open-PR SQLx scan; retry with stable refs")
    final_prs = snapshot_open_prs(
        github_pages(f"{root}/pulls?state=open&per_page=100", token)
    )
    if final_prs != initial_prs:
        raise AuditError("Open PR membership/head/base changed during SQLx scan; retry")
    return {
        "repository": repo,
        "main_sha": main_sha,
        "open_prs_scanned": len(observed),
        "open_pr_numbers": sorted(initial_prs),
        "collisions": scan_conflicts(main_files, observed),
        "cross_version_identical_sql": scan_cross_version_identical_sql(main_files, observed),
        "notice": (
            "Read-only candidate conflicts from changed files of OPEN PRs; "
            "not a merged-tree or applied-database verification. "
            "Exact SQL blob reuse under different versions is also reported; "
            "different SQL bytes may still be semantically identical. "
            "Main and PR head/base snapshots checked twice, not atomic. "
            "Inherited, removed, and renamed paths require integration review."
        ),
    }


def focus_report_on_pr(report, pr_number):
    """Only report global SQLx collisions involving one open PR.

    The scan itself remains global and uses the same double-checked refs.
    This is a convenience for triage, not a merged-tree or installed-DB gate.
    """
    if (isinstance(pr_number, bool) or not isinstance(pr_number, int)
            or pr_number <= 0):
        raise AuditError("Focus PR number must be a positive integer")
    members = report.get("open_pr_numbers")
    if not isinstance(members, list) or pr_number not in members:
        raise AuditError("Focus PR is not in the verified OPEN PR snapshot")
    owner = f"#{pr_number}"
    numeric = [
        item for item in report["collisions"]
        if any(owner in variant["owners"] for variant in item["variants"])
    ]
    repeated = [
        item for item in report["cross_version_identical_sql"]
        if any(
            owner in record["owners"]
            for version in item["versions"]
            for record in version["files"]
        )
    ]
    return {
        **report,
        "focus_pr": pr_number,
        "global_numeric_collisions": len(report["collisions"]),
        "global_cross_version_reuses": len(report["cross_version_identical_sql"]),
        "collisions": numeric,
        "cross_version_identical_sql": repeated,
        "notice": (
            report["notice"] + f" Focus is only changed migration files "
            f"owned by open PR #{pr_number}; this is not a reservation, "
            "permission to rename, or evidence of an approved DB ledger."
        ),
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    inventory = commands.add_parser("inventory", help="Compare installed SQLx history with local SQL")
    source = inventory.add_mutually_exclusive_group(required=True)
    source.add_argument("--csv", type=Path, help="Previously exported read-only CSV")
    source.add_argument("--from-psql", action="store_true", help="Run one read-only COPY TO STDOUT")
    inventory.add_argument("--migrations", type=Path, default=Path("migrations"))
    inventory.add_argument(
        "--save-raw-csv", type=Path,
        help="Store the complete SELECT-only ledger in a new file inside an operator-owned 0700 directory (only with --from-psql)",
    )
    scan = commands.add_parser("scan-open-prs", help="Read-only GitHub version collision report")
    scan.add_argument("--repo", default="calex794/tor")
    scan.add_argument("--focus-pr", type=int, help="Only report collisions involving one verified open PR")
    scan.add_argument("--fail-on-conflict", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.command == "inventory":
            if args.save_raw_csv is not None and not args.from_psql:
                raise AuditError("--save-raw-csv requires --from-psql")
            expected = local_migrations(args.migrations)
            text = read_from_psql() if args.from_psql else args.csv.read_text(encoding="utf-8")
            name, installed = parse_inventory(text)
            report = evaluate_history(expected, name, installed)
            if args.save_raw_csv is not None:
                save_private_inventory_csv(args.save_raw_csv, text)
            blocked = report["status"] == "BLOCK"
        else:
            report = scan_live(args.repo)
            if args.focus_pr is not None:
                report = focus_report_on_pr(report, args.focus_pr)
            blocked = args.fail_on_conflict and bool(
                report["collisions"] or report["cross_version_identical_sql"]
            )
        print(json.dumps(report, indent=2, ensure_ascii=False))
        return 2 if blocked else 0
    except (AuditError, OSError, UnicodeError, KeyError, TypeError) as error:
        print(f"SQLx audit INCOMPLETE: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
