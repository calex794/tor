#!/usr/bin/env python3
"""Check real CSV replay and rollback only on disposable local PostgreSQL."""
import csv
import os
from pathlib import Path
import secrets
import subprocess
import sys
from tempfile import TemporaryDirectory
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import taxonomy_import_csv as importer


def scalar(url, sql):
    result = subprocess.run(
        ["psql", "-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-d", url, "-c", sql],
        text=True, capture_output=True, timeout=30, check=False,
    )
    if result.returncode:
        raise AssertionError(result.stderr[-1200:])
    return int(result.stdout.strip())


def make_csv(root, name, rows):
    path = root / name
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(("kind", "label", "aliases"))
        writer.writerows(rows)
    return path


def apply(path, expected_success=True):
    status = importer.main([
        str(path), "--curator", "ci-smoke", "--confirm-db", "tor_ci", "--apply",
    ])
    if (status == 0) != expected_success:
        raise AssertionError(
            f"unexpected CSV result {status}; expected success={expected_success}"
        )


def main():
    url = os.environ.get("MIGRATOR_URL", "")
    host = urlsplit(url).hostname
    # Guard against mistakenly running any test against a real or remote DB.
    if importer.database_name(url) != "tor_ci" or host not in (
        "127.0.0.1", "localhost",
    ):
        raise ValueError("CSV smoke requires the local disposable tor_ci")
    os.environ["TOR_TAXONOMY_ADMIN_URL"] = url
    suffix = secrets.token_hex(6)
    tag = f"Ёжик{suffix}"
    genre = f"FantasyCsv{suffix}"
    synonym = f"SynonymCsv{suffix}"
    synonym_zh = f"同义{suffix}"
    more_synonyms = f"ExtraCsv{suffix}"
    rollback_first = f"RollbackFirst{suffix}"
    rollback_second = f"RollbackSecond{suffix}"
    quote = importer.sql_literal

    def audit_count():
        return scalar(url, f"""
            SELECT count(*) FROM book_taxonomy_curation_events
            WHERE term_id IN (
                SELECT id FROM book_taxonomy_terms
                WHERE (kind='tag' AND label={quote(tag)})
                   OR (kind='genre' AND label={quote(genre)})
            )
        """)

    with TemporaryDirectory() as directory:
        root = Path(directory)
        approved = make_csv(root, "approved.csv", [
            ("tag", tag, f"{synonym}|{synonym_zh}"),
            ("genre", genre, ""),
        ])
        apply(approved)
        initial_events = audit_count()
        if initial_events != 4:
            raise AssertionError(f"expected 2 creates + 2 aliases: {initial_events}")
        apply(approved)
        if audit_count() != initial_events:
            raise AssertionError("identical replay created duplicate audit events")
        if scalar(url, f"SELECT count(*) FROM book_taxonomy_terms WHERE kind='tag' AND label={quote(tag)}") != 1:
            raise AssertionError("identical replay duplicated canonical term")

        extended = make_csv(root, "extended.csv", [
            ("tag", tag, f"{synonym}|{synonym_zh}|{more_synonyms}"),
            ("genre", genre, ""),
        ])
        apply(extended)
        if audit_count() != initial_events + 1:
            raise AssertionError("adding one reviewed synonym changed other audit records")

        conflicting = make_csv(root, "conflict.csv", [
            ("tag", rollback_first, ""),
            ("tag", rollback_second, synonym),  # Existing alias owned by tag.
        ])
        apply(conflicting, expected_success=False)
        sql = (
            "SELECT count(*) FROM book_taxonomy_terms WHERE label IN "
            f"({quote(rollback_first)},{quote(rollback_second)})"
        )
        if scalar(url, sql) != 0:
            raise AssertionError("an earlier row of a conflicting batch was committed")
        if audit_count() != initial_events + 1:
            raise AssertionError("failed import changed existing curation events")

        fullwidth_case = make_csv(root, "fullwidth-spelling.csv", [
            ("genre", f"ＦａｎｔａｓｙＣｓｖ{suffix}", ""),
        ])
        apply(fullwidth_case, expected_success=False)
        if audit_count() != initial_events + 1:
            raise AssertionError("NFKC-equivalent label was accepted as a rename")

        incompatible_case = make_csv(root, "different-spelling.csv", [
            ("tag", tag.replace("Ё", "Е"), ""),
        ])
        apply(incompatible_case, expected_success=False)
        if audit_count() != initial_events + 1:
            raise AssertionError("alternative canonical spelling was accepted")

    print("CSV PostgreSQL: identical replay, one new alias, rollback and collision PASS")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, AssertionError, OSError, subprocess.TimeoutExpired) as err:
        print(f"taxonomy CSV PostgreSQL smoke: {err}", file=sys.stderr)
        raise SystemExit(2) from err
