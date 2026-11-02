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


def lookup(kind, value):
    script = Path(__file__).resolve().parents[1] / "taxonomy_admin.py"
    done = subprocess.run(
        [sys.executable, str(script), "lookup", kind, value],
        env=os.environ.copy(), text=True, capture_output=True,
        timeout=30, check=False,
    )
    if done.returncode:
        raise AssertionError(f"curator lookup failed: {done.stderr[-400:]}")
    return done.stdout.strip()


def unmapped():
    script = Path(__file__).resolve().parents[1] / "taxonomy_admin.py"
    done = subprocess.run(
        [sys.executable, str(script), "unmapped", "--limit", "500"],
        env=os.environ.copy(), text=True, capture_output=True,
        timeout=30, check=False,
    )
    if done.returncode:
        raise AssertionError(f"curator review queue failed: {done.stderr[-400:]}")
    return done.stdout.splitlines()


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
        # Existing published facets must be explicitly reviewed; CSV approval
        # retains the candidate ID and writes exactly one audit decision.
        candidate = f"ReviewCsv{suffix}"
        # Wrap INSERT RETURNING in a SELECT to avoid psql's INSERT 0 1 tag
        # polluting the numeric scalar output.
        candidate_id = scalar(
            url,
            "WITH inserted AS ("
            "INSERT INTO book_taxonomy_terms(kind,label,search_key,status) "
            f"VALUES('tag',{quote(candidate)},book_taxonomy_key({quote(candidate)}),'candidate') "
            "RETURNING id) SELECT id FROM inserted",
        )
        # Curator diagnostics may report pending candidates; author search may
        # not. This verifies the ID and status before the explicit review.
        if f"{candidate_id}|tag|{candidate}|candidate" not in lookup("tag", candidate):
            raise AssertionError("review candidate missing from curator diagnostics")
        # Only a public, non-held label can enter the review queue.
        hidden = f"PrivateOnly{suffix}"
        scalar(
            url,
            "WITH created AS (INSERT INTO books"
            "(source_identity,target_language,title,visibility,tags) "
            f"VALUES({quote('ci-public:'+suffix)},'ru','Review public','public',ARRAY[{quote(candidate)}]) "
            "RETURNING id) SELECT id FROM created",
        )
        scalar(
            url,
            "WITH created AS (INSERT INTO books"
            "(source_identity,target_language,title,visibility,tags) "
            f"VALUES({quote('ci-private:'+suffix)},'ru','Review draft','draft',ARRAY[{quote(hidden)}]) "
            "RETURNING id) SELECT id FROM created",
        )
        expected = f"tag|{candidate}|{importer.key(candidate)}|1|1|{candidate_id}|candidate"
        entries = unmapped()
        if expected not in entries:
            raise AssertionError("review queue omitted candidate label, count or ID")
        if any(hidden in entry for entry in entries):
            raise AssertionError("private draft leaked into public review queue")
        reviewed = make_csv(root, "reviewed-candidate.csv", [("tag", candidate, "")])
        apply(reviewed)
        if any(candidate in entry for entry in unmapped()):
            raise AssertionError("approved candidate remained in review queue")
        if scalar(url, f"SELECT id FROM book_taxonomy_terms WHERE label={quote(candidate)} AND status='active'") != candidate_id:
            raise AssertionError("CSV approval failed or changed the candidate ID")
        apply(reviewed)
        if scalar(url, f"SELECT count(*) FROM book_taxonomy_curation_events WHERE term_id={candidate_id} AND action='approve_term'") != 1:
            raise AssertionError("CSV candidate approval was not audited once")
        approved = make_csv(root, "approved.csv", [
            ("tag", tag, f"{synonym}|{synonym_zh}"),
            ("genre", genre, ""),
        ])
        apply(approved)
        # The CLI must look up fullwidth alphabetical prefixes correctly.
        if genre not in lookup("genre", f"ＦａｎｔａｓｙＣｓｖ{suffix}"):
            raise AssertionError("NFKC-equivalent prefix could not find approved genre")
        # Unicode NFKC turns these characters into LIKE wildcards.
        # They must never enumerate the whole canonical/alias dictionary.
        for wildcard in ("％", "＿", "＼"):
            if lookup("genre", wildcard):
                raise AssertionError("normalized wildcard expanded curator lookup")
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
