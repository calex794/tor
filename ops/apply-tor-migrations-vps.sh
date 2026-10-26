#!/usr/bin/env bash
# TOR I-05: operator-gated FIRST PostgreSQL schema migration on the TOR VPS.
# No Rust build or HTTP deployment. Run as root, use tor_migrator only.
set -Eeuo pipefail
umask 077

release_sha=${1:-}
if [[ $(id -u) -ne 0 || ! $release_sha =~ ^[0-9a-f]{40}$ || $# -ne 1 ]]; then
  echo "Usage (as root): bash ops/apply-tor-migrations-vps.sh <STAGED_GIT_SHA>" >&2
  exit 2
fi
if ! systemctl is-active --quiet postgresql@18-main.service; then
  echo "Refusing: PostgreSQL 18/main is inactive" >&2
  exit 1
fi
if ! systemctl is-active --quiet tor-web.service; then
  echo "Refusing: current TOR web service is inactive" >&2
  exit 1
fi

release="/opt/tor/releases/$release_sha"
migrator="$release/tor-migrate"
checksums="$release/tor-migrate.sha256"
if [[ ! -d $release || -L $release || $(readlink -f "$release") != "$release" ||
      ! -f $migrator || -L $migrator || ! -x $migrator || ! -f $checksums ]]; then
  echo "Refusing: expected staged SQLx binary and checksum at $release" >&2
  exit 1
fi
(cd "$release" && sha256sum -c --status tor-migrate.sha256) || {
  echo "Refusing: staged tor-migrate checksum mismatch" >&2
  exit 1
}

secret=/etc/tor/tor-migrator.env
if [[ -L $secret || ! -f $secret ||
      $(stat -c '%U:%G:%a' "$secret") != root:root:600 ]]; then
  echo "Refusing: /etc/tor/tor-migrator.env must be a root-only 0600 regular file" >&2
  exit 1
fi
set -a
# Root-owned, mode 0600, created by the TOR database bootstrap.
# shellcheck disable=SC1091
. "$secret"
set +a
if [[ ! ${DATABASE_URL:-} =~ ^postgresql://tor_migrator:[0-9a-f]{64}@127[.]0[.]0[.]1:5432/tor[?]sslmode=disable$ ]]; then
  echo "Refusing: tor_migrator URL must point to the local TOR database" >&2
  exit 1
fi

database_owner=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -Atqc \
  "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname = 'tor'")
if [[ $database_owner != tor_migrator ]]; then
  echo "Refusing: tor database is not owned by tor_migrator" >&2
  exit 1
fi
table_count=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM pg_tables WHERE schemaname='public' AND tablename <> '_sqlx_migrations'")
if [[ $table_count != 0 && $table_count -lt 17 ]]; then
  echo "Unexpected partial schema ($table_count tables), refusing migration" >&2
  exit 1
fi
if [[ $table_count != 0 ]]; then
  has_history=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
    "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
  if [[ $has_history != t ]]; then
    echo "Refusing to upgrade database without SQLx history" >&2
    exit 1
  fi
fi
if [[ $(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -Atqc 'SHOW server_version_num') != 18???? ]]; then
  echo "Refusing: expected PostgreSQL 18" >&2
  exit 1
fi

backup_dir=/var/backups/tor
install -d -o root -g root -m 0700 "$backup_dir"
backup=$(mktemp "$backup_dir/tor-before-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXXXX.dump")
chmod 0600 "$backup"
echo "Creating local pre-migration backup: $backup"
runuser -u postgres -- pg_dump -Fc --no-owner --no-privileges -d tor > "$backup"
pg_restore -l "$backup" >/dev/null
echo "Backup archive readable; running canonical SQLx migrations (initial or additive)"

# This binary checks current_user == tor_migrator and target DB == tor.
# On SQLx failure do not touch the currently running tor-web demo service.
migrate_result=$("$migrator")
printf '%s\n' "$migrate_result"
expected=$(printf '%s\n' "$migrate_result" |
  sed -n 's/^TOR_SQLX_APPLIED_OK \([0-9][0-9]*\)\/\1$/\1/p')
if [[ ! $expected =~ ^[0-9]+$ ]] || (( expected < 9 )); then
  echo "SQLx migrator did not verify all embedded versions" >&2
  exit 1
fi

# The app needs to READ migration readiness, but must never edit SQLx history.
# Default app DML grants made by the bootstrap otherwise apply to this table.
runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -q -d tor <<'SQL'
REVOKE INSERT, UPDATE, DELETE ON TABLE public._sqlx_migrations FROM tor_app;
GRANT SELECT ON TABLE public._sqlx_migrations TO tor_app;
SQL
role_privileges=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT has_table_privilege('tor_app','public._sqlx_migrations','SELECT')::text
          || ',' ||
          has_table_privilege('tor_app','public._sqlx_migrations','INSERT')::text
          || ',' ||
          has_table_privilege('tor_app','public._sqlx_migrations','UPDATE')::text
          || ',' ||
          has_table_privilege('tor_app','public._sqlx_migrations','DELETE')::text")
if [[ $role_privileges != true,false,false,false ]]; then
  echo "Refusing: tor_app can modify SQLx migration history" >&2
  exit 1
fi
echo "MIGRATION_HISTORY_ACCESS_OK: tor_app can SELECT but cannot modify migration history"

versions=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations WHERE success = TRUE")
all_versions=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations")
source_versions=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations WHERE success = TRUE
   AND version IN (202610120001,202610120002,202610120003,202610120004,
                   202610120005,202610120006,202610130001,202610130002,
                   202610130003)")
tables=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM pg_tables WHERE schemaname='public' AND tablename <> '_sqlx_migrations'")
if [[ $versions != "$expected" || $all_versions != "$expected" ||
      $source_versions != 9 || $tables -lt 17 ]]; then
  echo "SQLx check failed: applied=$versions expected=$expected donor=$source_versions tables=$tables" >&2
  exit 1
fi

echo "MIGRATIONS_OK: $versions/$expected SQLx versions, $tables domain tables"
echo "BACKUP_OK: $backup (local only; off-server backup still required)"
echo "=== PostgreSQL and Rust service ==="
systemctl is-active postgresql@18-main.service
systemctl is-active tor-web.service
curl --fail --silent --show-error --max-time 5 http://127.0.0.1:3000/readyz >/dev/null
echo "TOR_DEMO_HEALTH_OK"
echo "=== Memory ==="
free -h
echo "Current TOR web remains RAM demo; database is prepared for separate runtime cutover"
