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
  "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")
if [[ $table_count != 0 ]]; then
  echo "Refusing: TOR must have 0 public tables before first install (has $table_count)" >&2
  echo "Use documented upgrade migration after inspecting the existing schema" >&2
  exit 1
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
echo "Backup archive readable; applying nine original SQLx migrations"

# This binary checks current_user == tor_migrator and target DB == tor.
# On SQLx failure do not touch the currently running tor-web demo service.
"$migrator"

versions=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations WHERE success = TRUE")
all_versions=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations")
tables=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM pg_tables WHERE schemaname='public' AND tablename <> '_sqlx_migrations'")
if [[ $versions != 9 || $all_versions != 9 || $tables != 17 ]]; then
  echo "Migration check failed: success=$versions total=$all_versions domain_tables=$tables" >&2
  echo "Do not enable TOR PostgreSQL mode before investigating" >&2
  exit 1
fi

echo "MIGRATIONS_OK: 9/9 SQLx versions, 17 domain tables"
echo "BACKUP_OK: $backup (local only; off-server backup still required)"
echo "=== PostgreSQL and Rust service ==="
systemctl is-active postgresql@18-main.service
systemctl is-active tor-web.service
curl --fail --silent --show-error --max-time 5 http://127.0.0.1:3000/readyz >/dev/null
echo "TOR_DEMO_HEALTH_OK"
echo "=== Memory ==="
free -h
echo "Current TOR web remains RAM demo; database is prepared for separate runtime cutover"
