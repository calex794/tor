#!/usr/bin/env python3
"""AUTH-08: synthetic browser cookies retained across isolated release/restore drills.

Never issue or read actual customer cookies. Only disposable runner databases
are accepted by the calling S061 drill and the standalone I-11 entry point.
No raw tokens, cookie headers, password hashes or digests enter reports.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
import urllib.error
import urllib.request


KINDS = ("live", "revoked", "expired")
COOKIE_NAME = "tor-dev-session"
RESTORE_URL = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_restore_ci"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(token):
    return hashlib.sha256(token.encode("ascii")).hexdigest()


def session_snapshot(sql, database, witness):
    hashes = [digest(witness["tokens"][kind]) for kind in KINDS]
    values = ",".join(
        f"('{kind}',decode('{value}','hex'))"
        for kind, value in zip(KINDS, hashes)
    )
    rows = sql(f"""
        WITH selected(kind, token_hash) AS (VALUES {values})
        SELECT jsonb_object_agg(selected.kind,
            jsonb_build_array(s.session_id::text,s.user_id,
                extract(epoch FROM s.created_at),
                extract(epoch FROM s.expires_at),
                extract(epoch FROM s.revoked_at),
                e.event_kind,e.agent_family))
        FROM selected JOIN sessions s USING(token_hash)
        LEFT JOIN account_login_events e ON e.session_id=s.session_id AND e.user_id=s.user_id
    """, database)
    result = json.loads(rows)
    require(isinstance(result, dict) and set(result) == set(KINDS),
            "AUTH08 expected three retained session facts")
    return result


def seed_sessions(sql, database, email):
    require(email in ("s061-catalog@example.invalid", "i11-restored-buyer@example.invalid"),
            "AUTH08 unexpected fixture account")
    tokens = {kind: secrets.token_urlsafe(32) for kind in KINDS}
    values = []
    for kind in KINDS:
        expiry = "now()-INTERVAL '1 minute'" if kind == "expired" else "now()+INTERVAL '6 days'"
        revoked = "now()" if kind == "revoked" else "NULL::timestamptz"
        values.append(f"(decode('{digest(tokens[kind])}','hex'),{expiry},{revoked})")
    sql(f"""
        WITH seeded AS (
            INSERT INTO sessions(user_id,token_hash,created_at,expires_at,revoked_at)
            SELECT u.id,v.token_hash,now()-INTERVAL '1 day',v.expires_at,v.revoked_at
            FROM users u CROSS JOIN (VALUES {','.join(values)}) v(token_hash,expires_at,revoked_at)
            WHERE u.email_normalized='{email}'
            RETURNING user_id,session_id
        )
        INSERT INTO account_login_events(session_id,user_id,agent_family,event_kind)
        SELECT session_id,user_id,'chrome','password_login' FROM seeded
    """, database)
    witness = {"tokens": tokens}
    witness["snapshot"] = session_snapshot(sql, database, witness)
    require(witness["snapshot"]["live"][4] is None
            and witness["snapshot"]["revoked"][4] is not None,
            "AUTH08 incorrect revocation fixture")
    require(witness["snapshot"]["live"][3] > witness["snapshot"]["expired"][3],
            "AUTH08 incorrect expiration fixture")
    return witness


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request(port, token, active=False):
    require(port in (3091, 3092, 3096), "AUTH08 refuses non-drill HTTP port")
    require(re.fullmatch(r"[A-Za-z0-9_-]{43}", token) is not None,
            "AUTH08 invalid synthetic token shape")
    headers = {"Cookie": f"{COOKIE_NAME}={token}"}
    if active:
        headers.update({
            "Sec-Fetch-Site": "same-origin",
            "Sec-Fetch-Mode": "navigate",
            "Sec-Fetch-Dest": "document",
            "Sec-Fetch-User": "?1",
        })
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    req = urllib.request.Request(f"http://127.0.0.1:{port}/me", headers=headers)
    try:
        response = opener.open(req, timeout=5)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return response.status, response.headers.get("Location"), response.headers.get_all("Set-Cookie", [])


def assert_sessions(port, witness):
    for kind in KINDS:
        status, location, cookies = request(port, witness["tokens"][kind])
        if kind == "live":
            require(status == 200, "AUTH08 live cookie lost access to /me")
        else:
            require(status == 303 and location == "/login",
                    f"AUTH08 {kind} cookie was not rejected by /me")
        require(not any(value.startswith(COOKIE_NAME + "=") for value in cookies),
                "AUTH08 passive request unexpectedly issued a session cookie")


def assert_restored_renewal(sql, witness):
    assert_sessions(3096, witness)
    before = witness["snapshot"]
    require(session_snapshot(sql, RESTORE_URL, witness) == before,
            "AUTH08 restored session facts differ before activity")
    status, _, cookies = request(3096, witness["tokens"]["live"], active=True)
    require(status == 200, "AUTH08 restored live cookie failed active navigation")
    prefix = f'{COOKIE_NAME}={witness["tokens"]["live"]};'
    issued = [value for value in cookies if value.startswith(prefix)]
    require(len(issued) == 1, "AUTH08 restored navigation must renew the original cookie")
    age = re.search(r"(?:^|; )Max-Age=(\d+)(?:;|$)", issued[0])
    require(age is not None and 2_591_990 <= int(age[1]) <= 2_592_000,
            "AUTH08 restored session did not renew to thirty days")
    after = session_snapshot(sql, RESTORE_URL, witness)
    require(after["live"][:3] == before["live"][:3]
            and after["live"][4:] == before["live"][4:]
            and after["live"][3] - before["live"][3] > 23 * 86_400,
            "AUTH08 renewal did not preserve the original session identity")
    for kind in ("revoked", "expired"):
        status, location, cookies = request(3096, witness["tokens"][kind], active=True)
        require(status == 303 and location == "/login"
                and not any(value.startswith(COOKIE_NAME + "=") for value in cookies),
                f"AUTH08 {kind} cookie regained access during activity")
        require(after[kind] == before[kind], f"AUTH08 {kind} fact changed")
    require(session_snapshot(sql, RESTORE_URL, witness) == after,
            "AUTH08 rejected activity mutated session facts")



def assert_stale_restore_invalidation(sql, source, witness):
    """AUTH-08b: prove the safety policy on an isolated, stale archive.

    The original continuity assertions run first. Only then revoke the
    synthetic live cookie on the source, making the restored copy stale.
    Never apply the bulk update to the source or to a deployed database.
    The operational recovery gate is separate from this CI experiment.
    """
    require(source == "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
            "AUTH08B refuses non-fixture source")
    token = witness["tokens"]["live"]
    original = session_snapshot(sql, source, witness)
    require(original == witness["snapshot"],
            "AUTH08B source changed before deliberate fixture revocation")
    changed = sql(f"""
        WITH ended AS (
            UPDATE sessions SET revoked_at=clock_timestamp()
            WHERE token_hash=decode('{digest(token)}','hex') AND revoked_at IS NULL
            RETURNING 1
        ) SELECT count(*) FROM ended
    """, source)
    require(changed == "1", "AUTH08B expected exactly one post-backup revocation")
    source_after = session_snapshot(sql, source, witness)
    require(source_after["live"][4] is not None
            and all(source_after[kind] == original[kind] for kind in ("revoked", "expired")),
            "AUTH08B source revocation witness differs")
    # The isolated restore intentionally accepts the now-revoked source
    # cookie. This is the exact risk when recovery loses security events.
    require(request(3096, token)[0] == 200,
            "AUTH08B expected stale restored cookie before invalidation")
    print("AUTH08B_STALE_BACKUP_REVOCATION_GAP_REPRODUCED")

    # Use the exact same atomic mutation as the offline operator step.
    # The production-only target/host guard is never bypassed by that CLI.
    from tor_recovery_auth import invalidation_sql, operator_guard

    revoke_restored = invalidation_sql()
    restored_before = session_snapshot(sql, RESTORE_URL, witness)

    def expect_refusal(statement, marker, superuser=False):
        # The superuser case uses the existing disposable CI identity so
        # access to pg_control_system cannot mask the actual guard check.
        target = (["--host=127.0.0.1", "--port=5432", "--username=postgres",
                   "--dbname=tor_restore_ci"] if superuser else [RESTORE_URL])
        result = subprocess.run(
            ["psql", *target, "-X", "-A", "-t", "-q",
             "-v", "ON_ERROR_STOP=1"],
            input=statement, text=True, capture_output=True, timeout=25,
        )
        require(result.returncode != 0 and marker in result.stderr,
                "AUTH08C expected guarded SQL refusal")
        require(session_snapshot(sql, RESTORE_URL, witness) == restored_before,
                "AUTH08C failed transaction changed restored sessions")
        require(request(3096, token)[0] == 200,
                "AUTH08C refused operation invalidated a live session")

    # Execute the real operator guard against this deliberately wrong target:
    # TCP, the ordinary CI port and unapproved cluster identity.
    expect_refusal(
        invalidation_sql(operator_guard("tor_restore_ci", "1", 15432)),
        "AUTH08C isolated promoted database guard failed", superuser=True,
    )
    print("AUTH08C_WRONG_TARGET_REFUSED")

    # Force a failure after the actual UPDATE and verification, but before
    # COMMIT. The connection must roll back and the original cookie survive.
    require(revoke_restored.count("COMMIT;") == 1,
            "AUTH08C unexpected transaction boundary")
    expect_refusal(
        revoke_restored.replace(
            "COMMIT;",
            "DO $ BEGIN RAISE EXCEPTION 'AUTH08C forced rollback'; END; $; COMMIT;",
        ),
        "AUTH08C forced rollback",
    )
    print("AUTH08C_INVALIDATION_ROLLBACK_OK")
    active_count = """
        SELECT count(*) FROM sessions
        WHERE revoked_at IS NULL AND expires_at>now()
    """
    before_count = int(sql(active_count, RESTORE_URL))
    require(before_count > 0, "AUTH08B missing restored live sessions")
    require(int(sql(revoke_restored, RESTORE_URL)) == before_count,
            "AUTH08B invalidation did not revoke every live restored session")
    require(sql(active_count, RESTORE_URL) == "0"
            and sql(revoke_restored, RESTORE_URL) == "0",
            "AUTH08B invalidation is not complete and idempotent")

    def assert_old_denied():
        for kind in KINDS:
            for active in (False, True):
                status, location, cookies = request(3096, witness["tokens"][kind], active)
                require(status == 303 and location == "/login"
                        and not any(value.startswith(COOKIE_NAME + "=") for value in cookies),
                        f"AUTH08B {kind} cookie survived invalidation")

    assert_old_denied()
    # New synthetic sessions can be established after invalidation.
    fresh = seed_sessions(sql, RESTORE_URL, "i11-restored-buyer@example.invalid")
    assert_sessions(3096, fresh)
    assert_old_denied()
    require(session_snapshot(sql, source, witness) == source_after,
            "AUTH08B restored invalidation mutated source sessions")
    print("AUTH08B_STALE_RESTORE_SESSION_INVALIDATION_OK")


def main():
    # Import only for the standalone I-11 command, after module initialization.
    # S061 imports the seed/check functions directly inside its guarded drill.
    from s061_cutover_pg_smoke import EXPECTED, require_disposable, sql

    runner = require_disposable(dict(os.environ)).resolve()
    require(len(sys.argv) == 3 and sys.argv[1] in ("seed", "check"),
            "AUTH08 expected seed/check and private witness path")
    path = Path(sys.argv[2])
    require(path.name == "auth-sessions.json"
            and path.parent.name.startswith("tor-pg-restore.")
            and path.parent.resolve().parent == runner
            and path.parent.is_dir() and not path.parent.is_symlink()
            and stat.S_IMODE(path.parent.stat().st_mode) == 0o700,
            "AUTH08 requires private restore workspace")
    source = EXPECTED["TEST_DATABASE_URL"]
    if sys.argv[1] == "seed":
        witness = seed_sessions(sql, source, "i11-restored-buyer@example.invalid")
        with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as handle:
            json.dump(witness, handle)
    else:
        require(not path.is_symlink() and path.is_file()
                and stat.S_IMODE(path.stat().st_mode) == 0o600,
                "AUTH08 witness must be private")
        witness = json.loads(path.read_text())
        assert_restored_renewal(sql, witness)
        require(session_snapshot(sql, source, witness) == witness["snapshot"],
                "AUTH08 restored activity changed original database")
        assert_stale_restore_invalidation(sql, source, witness)


if __name__ == "__main__":
    main()
