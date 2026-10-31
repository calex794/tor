#!/usr/bin/env python3
"""Fail closed on migration-bearing PRs while issue #235 SQLx HOLD is open.

This is a read-only branch-merge policy check. It does not inspect any retained
database, make owner attestations, modify a migration, or authorize deployment.
"""
import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request

REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
ISSUE_NUMBER = 235
SHA = re.compile(r"^[0-9a-f]{40}$")


class HoldCheckError(ValueError):
    """The release policy is still blocked or could not be verified safely."""


def github_json(path, token):
    request = urllib.request.Request(
        "https://api.github.com" + path,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "tor-sqlx-merge-hold-ci",
            "X-GitHub-Api-Version": "2022-11-28",
            "Authorization": "Bearer " + token,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)
    except (OSError, UnicodeError, ValueError) as error:
        # Never echo GitHub diagnostics, token, or request headers in CI logs.
        raise HoldCheckError("GitHub read-only HOLD evidence unavailable") from error


def pr_snapshot(repo, pr_number, token):
    """Read a mutable PR identity; caller must check it against event SHAs."""
    if not REPOSITORY.fullmatch(repo) or any(
        part in {".", ".."} for part in repo.split("/")
    ):
        raise HoldCheckError("Invalid repository identity")
    if isinstance(pr_number, bool) or not isinstance(pr_number, int) or pr_number <= 0:
        raise HoldCheckError("Invalid pull request number")
    pr = github_json(f"/repos/{repo}/pulls/{pr_number}", token)
    if not isinstance(pr, dict) or pr.get("number") != pr_number or pr.get("state") != "open":
        raise HoldCheckError("PR identity unavailable or no longer open")
    head, base = pr.get("head"), pr.get("base")
    if not isinstance(head, dict) or not isinstance(base, dict):
        raise HoldCheckError("PR head/base identity unavailable")
    sha_pair = (head.get("sha"), base.get("sha"))
    if any(not isinstance(sha, str) or not SHA.fullmatch(sha) for sha in sha_pair):
        raise HoldCheckError("Invalid PR head/base SHA")
    return sha_pair


def changed_pr_files(repo, pr_number, token):
    if not REPOSITORY.fullmatch(repo):
        raise HoldCheckError("Invalid repository identity")
    if isinstance(pr_number, bool) or not isinstance(pr_number, int) or pr_number <= 0:
        raise HoldCheckError("Invalid pull request number")
    files = []
    for page in range(1, 11):
        chunk = github_json(
            f"/repos/{repo}/pulls/{pr_number}/files?per_page=100&page={page}", token
        )
        if not isinstance(chunk, list):
            raise HoldCheckError("PR files response is not a list")
        for item in chunk:
            if not isinstance(item, dict) or not isinstance(item.get("filename"), str):
                raise HoldCheckError("Invalid PR changed-file entry")
            files.append(item)
        if len(chunk) < 100:
            if not files:
                raise HoldCheckError("Empty PR file inventory is incomplete evidence")
            return files
    raise HoldCheckError("PR changed-file listing exceeds the verified page limit")


def changes_sqlx(files):
    for item in files:
        for path in (item.get("filename"), item.get("previous_filename")):
            if isinstance(path, str) and path.startswith("migrations/") and path.endswith(".sql"):
                return True
    return False


def enforce_hold(repo, pr_number, token, expected_head, expected_base):
    if not token:
        raise HoldCheckError("Read-only GitHub token required")
    if any(
        not isinstance(sha, str) or not SHA.fullmatch(sha)
        for sha in (expected_head, expected_base)
    ):
        raise HoldCheckError("Exact pull request event head/base SHAs required")
    expected = (expected_head, expected_base)
    before = pr_snapshot(repo, pr_number, token)
    if before != expected:
        raise HoldCheckError("PR head/base differs from the CI event; rerun checks")
    files = changed_pr_files(repo, pr_number, token)
    issue = None
    if changes_sqlx(files):
        issue = github_json(f"/repos/{repo}/issues/{ISSUE_NUMBER}", token)
        if not isinstance(issue, dict):
            raise HoldCheckError("Invalid SQLx HOLD issue response")
    # /pulls/{number}/files is not SHA-pinned. Even a docs-only PASS must
    # refuse a moved head/base, or it could omit a newly added SQL migration.
    if pr_snapshot(repo, pr_number, token) != expected:
        raise HoldCheckError("PR head/base changed during HOLD check; rerun checks")
    if issue is None:
        return "PASS: no changed SQLx migration files on the pinned PR head/base"
    if issue.get("state") != "closed" or issue.get("state_reason") != "completed":
        raise HoldCheckError(
            f"SQLx HOLD #{ISSUE_NUMBER} is not signed off as completed; "
            "migration-bearing PR cannot merge"
        )
    return (
        f"PASS: SQLx HOLD #{ISSUE_NUMBER} marked completed; "
        "still requires private installed-history evidence and owner approval"
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--pr", type=int, default=os.environ.get("TOR_PULL_REQUEST_NUMBER"))
    parser.add_argument("--expected-head", default=os.environ.get("TOR_PULL_REQUEST_HEAD_SHA"))
    parser.add_argument("--expected-base", default=os.environ.get("TOR_PULL_REQUEST_BASE_SHA"))
    args = parser.parse_args(argv)
    try:
        message = enforce_hold(
            args.repo or "", args.pr, os.environ.get("GITHUB_TOKEN"),
            args.expected_head, args.expected_base,
        )
        print(message)
        return 0
    except HoldCheckError as error:
        print(f"TOR_CI_REFUSED: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
