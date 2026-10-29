#!/usr/bin/env bash
# TOR I-12: read-only operator preflight for a staged production release.
# Does NOT migrate SQLx, change symlinks, restart services or edit secrets.
set -Eeuo pipefail

refuse() {
  printf 'TOR_PREFLIGHT_REFUSED: %s\n' "$*" >&2
  exit 1
}

if [[ $# -ne 1 ]]; then
  refuse 'pass exactly one full 40-character staged commit SHA'
fi
if [[ ! $1 =~ ^[0-9a-f]{40}$ ]]; then
  refuse 'invalid 40-character staged commit SHA'
fi
if [[ $(id -u) != 0 ]]; then
  refuse 'run as root on the dedicated TOR VPS'
fi
for command in curl python3 systemctl readlink runuser sha256sum basename; do
  command -v "$command" >/dev/null 2>&1 || refuse "required command missing: $command"
done

candidate_sha=$1
release_root=/opt/tor/releases
release="$release_root/$candidate_sha"
[[ -d "$release" && ! -L "$release" ]] || refuse 'candidate directory not staged'
[[ -x "$release/tor-web" && -x "$release/tor-migrate" ]] ||
  refuse 'candidate tor-web / tor-migrate executable missing'
[[ -f "$release/tor-migrate.sha256" && ! -L "$release/tor-migrate.sha256" ]] ||
  refuse 'candidate migrator checksum missing'
(cd "$release" && sha256sum -c --status tor-migrate.sha256) ||
  refuse 'staged migrator checksum mismatch'

active_path=$(readlink -f /opt/tor/current) || refuse 'active symlink cannot be read'
[[ "$active_path" == "$release_root/"* ]] || refuse 'active binary is outside release directory'
active_sha=$(basename "$active_path")
[[ "$active_sha" =~ ^[0-9a-f]{40}$ && -x "$active_path/tor-web" ]] ||
  refuse 'active release is not a valid executable SHA'
systemctl is-active --quiet tor-web.service ||
  refuse 'currently serving TOR is not active'
systemctl is-active --quiet postgresql@18-main.service ||
  refuse 'PostgreSQL 18 is not active'
[[ $(curl -fsS --max-time 5 http://127.0.0.1:3000/readyz) == ok ]] ||
  refuse 'currently serving TOR is not ready'

schema_marker=$(cat /var/lib/tor-release/schema-version) ||
  refuse 'root-verified schema marker missing'
[[ "$schema_marker" =~ ^[0-9]{12}$ ]] ||
  refuse 'root-verified schema marker malformed'
[[ $(cat /var/lib/tor-release/pg-activated) == PG_ACTIVATED ]] ||
  refuse 'verified PostgreSQL activation marker missing'
history=$(
  runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -At -d tor -c \
    "SELECT count(*)::text || ':' || max(version)::text || ':' ||
            count(*) FILTER (WHERE NOT success)::text
     FROM public._sqlx_migrations"
) || refuse 'cannot read actual SQLx history'
IFS=: read -r applied_count applied_latest failures <<< "$history"
[[ "$applied_count" =~ ^[0-9]+$ && "$applied_latest" =~ ^[0-9]{12}$ &&
   "$failures" == 0 && "$schema_marker" == "$applied_latest" ]] ||
  refuse 'root marker and actual successful SQLx history disagree'

# Verify latest SHA and migration manifest from public GitHub, read-only.
actual_main=$(
  curl -fsS --retry 2 --max-time 15 \
    -H 'Accept: application/vnd.github+json' \
    https://api.github.com/repos/calex794/tor/git/ref/heads/main |
  python3 -c '
import json
import re
import sys
v = json.load(sys.stdin)
sha = v.get("object", {}).get("sha", "")
if v.get("ref") != "refs/heads/main" or not re.fullmatch(r"[0-9a-f]{40}", sha):
    sys.exit(1)
print(sha)
'
) || refuse 'cannot verify GitHub main SHA'
[[ "$actual_main" == "$candidate_sha" ]] ||
  refuse 'staged candidate is not the current GitHub main'

required=$(
  curl -fsS --retry 2 --max-time 15 \
    -H 'Accept: application/vnd.github+json' \
    "https://api.github.com/repos/calex794/tor/contents/migrations?ref=$candidate_sha" |
  python3 -c '
import json
import re
import sys
rows = json.load(sys.stdin)
if not isinstance(rows, list):
    sys.exit(1)
versions = [
    int(match.group(1))
    for row in rows
    if isinstance(row, dict)
    for match in [re.fullmatch(r"(\d{12})_[a-z0-9_]+\.sql", row.get("name", ""))]
    if match is not None and row.get("type") == "file"
]
if len(versions) < 15 or len(versions) != len(set(versions)):
    sys.exit(1)
print(str(len(versions)) + ":" + str(max(versions)))
'
) || refuse 'cannot verify canonical SQLx migration manifest'
IFS=: read -r required_count required_latest <<< "$required"
[[ "$required_count" =~ ^[0-9]+$ && "$required_latest" =~ ^[0-9]{12}$ ]] ||
  refuse 'invalid candidate SQLx version list'
(( applied_count <= required_count )) ||
  refuse 'installed database has more migrations than candidate'
[[ "$schema_marker" < "$required_latest" || "$schema_marker" == "$required_latest" ]] ||
  refuse 'installed schema newer than selected candidate'
pending=$((required_count - applied_count))
if (( pending == 0 )) && [[ "$schema_marker" != "$required_latest" ]]; then
  refuse 'SQLx count matches but latest schema differs'
fi
if (( pending > 0 )) && [[ "$schema_marker" == "$required_latest" ]]; then
  refuse 'latest schema matches but some migrations are missing'
fi

printf 'TOR_ACTIVE_SHA %s\n' "$active_sha"
printf 'TOR_STAGED_SHA %s\n' "$candidate_sha"
printf 'TOR_SQLX_APPLIED %s/%s\n' "$applied_count" "$schema_marker"
printf 'TOR_SQLX_REQUIRED %s/%s\n' "$required_count" "$required_latest"
printf 'TOR_MIGRATIONS_PENDING %s\n' "$pending"
if (( pending > 0 )); then
  echo 'TOR_PREFLIGHT_STAGE_OK_MIGRATIONS_REQUIRED'
else
  echo 'TOR_PREFLIGHT_STAGE_OK_SCHEMA_CURRENT'
fi
echo 'TOR_PREFLIGHT_READ_ONLY: no migrations, restart or activation performed'
