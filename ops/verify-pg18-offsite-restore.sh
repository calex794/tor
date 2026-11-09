#!/usr/bin/env bash
# TOR I-11 / DR-01: restore a remote encrypted dump into an existing EMPTY
# disposable PostgreSQL database on a NON-PRODUCTION port, then check it.
set -Eeuo pipefail
umask 077
fail() { printf 'TOR_OFFSITE_RESTORE_REFUSED: %s\n' "$*" >&2; exit 1; }
[[ "$(id -u)" == 0 ]] || fail 'root required'
[[ "$#" == 0 ]] || fail 'no command-line arguments allowed'

config=/etc/tor/offsite-backup.env
[[ -f "$config" && ! -L "$config" &&
   "$(stat -c '%U:%G:%a' "$config")" == root:root:600 ]] ||
  fail 'configuration missing or unsafe'
# shellcheck disable=SC1091
source "$config"
[[ "${RESTIC_REPOSITORY:-}" == s3:https://* ]] || fail 'remote HTTPS S3 required'
[[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]] ||
  fail 'dedicated S3 credentials required'
[[ -n "${RESTIC_PASSWORD_FILE:-}" && -f "$RESTIC_PASSWORD_FILE" &&
   ! -L "$RESTIC_PASSWORD_FILE" &&
   "$(stat -c '%U:%G:%a' "$RESTIC_PASSWORD_FILE")" == root:root:600 ]] ||
  fail 'recovery password file unsafe'
export RESTIC_REPOSITORY RESTIC_PASSWORD_FILE AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY
for executable in restic pg_restore psql python3 flock; do
  command -v "$executable" >/dev/null || fail "missing $executable"
done

# Force a separate local test cluster on a distinct port and database.
# No production port, database or remote database may be used.
url="${TOR_DR_RESTORE_URL:-}"
[[ -n "$url" ]] || fail 'TOR_DR_RESTORE_URL required for disposable target'
# Passwords must NEVER appear in process argument lists. Use root-only .pgpass.
[[ -n "${PGPASSFILE:-}" && -f "$PGPASSFILE" && ! -L "$PGPASSFILE" &&
   "$(stat -c '%U:%G:%a' "$PGPASSFILE")" == root:root:600 ]] ||
  fail 'sandbox PGPASSFILE missing or unsafe'
# Parse user/host/port/database, refuse embedded passwords and connection options.
# This must not permit a production 5432 endpoint or a non-sandbox database.
read -r PGHOST PGPORT PGUSER PGDATABASE < <(
  TOR_DR_PARSE_URL="$url" python3 - <<'PY'
import os
from urllib.parse import urlparse
import re
import sys
u = urlparse(os.environ["TOR_DR_PARSE_URL"])
db = u.path.lstrip("/")
if (u.scheme not in ("postgres", "postgresql") or
    u.hostname not in ("127.0.0.1", "localhost") or
    u.username is None or not re.fullmatch(r"[a-z][a-z0-9_]{0,31}", u.username) or
    u.password is not None or
    u.port is None or not (15432 <= u.port <= 15999) or
    not re.fullmatch(r"tor_dr_[a-z0-9_]{1,32}", db) or
    u.query or u.fragment):
    sys.exit(1)
print(u.hostname, u.port, u.username, db)
PY
) || fail 'unsafe restore target'
[[ -n "${PGDATABASE:-}" && "${PGDATABASE}" == tor_dr_* ]] ||
  fail 'invalid restore target'
export PGHOST PGPORT PGUSER PGDATABASE PGPASSFILE
# The disposable database must already exist and have an EMPTY public schema.
[[ "$(psql -X -At -v ON_ERROR_STOP=1 -c \
  "SELECT count(*) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind IN ('r','p','v','m','f')" 2>/dev/null)" == 0 ]] ||
  fail 'restore target is nonempty or inaccessible'

exec 9>/run/lock/tor-offsite-restore.lock
flock -n 9 || fail 'another recovery test is running'

snapshot="$(restic snapshots --json --tag tor-pg18 | python3 -c '
import json,sys
items=json.load(sys.stdin)
items=[x for x in items if x.get("id") and "tor-pg18" in x.get("tags",[])]
if not items: sys.exit(1)
print(max(items,key=lambda x:x.get("time",""))["id"])
')" || fail 'no restorable PostgreSQL snapshot found'
[[ "$snapshot" =~ ^[0-9a-f]{64}$ ]] || fail 'unexpected snapshot identifier'

# The dump goes directly to PostgreSQL, not to a plaintext file.
# Pipeline failures are fatal because pipefail is set.
restic dump "$snapshot" tor-pg18.dump | \
  /usr/lib/postgresql/18/bin/pg_restore \
    --dbname="$PGDATABASE" --no-owner --no-acl --exit-on-error

# Only validate restored data in this disposable database.
psql -X -v ON_ERROR_STOP=1 -At -c \
  "SELECT count(*) FROM public._sqlx_migrations WHERE success" |
  grep -Eq '^[1-9][0-9]*$' || fail 'SQLx migrations missing'
psql -X -v ON_ERROR_STOP=1 -At -c \
  "SELECT count(*) FROM information_schema.tables WHERE table_schema='public' AND table_name='books'" |
  grep -qx 1 || fail 'books table missing'
echo TOR_OFFSITE_RESTORE_VERIFIED
