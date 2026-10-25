#!/usr/bin/env bash
# TOR I-05: PostgreSQL 18 on the dedicated Ubuntu 24.04, 961-MiB VPS.
# Does not create roles, credentials, databases, migrations or change nginx.
set -Eeuo pipefail
umask 022
if [[ $(id -u) -ne 0 ]]; then
  echo 'Run as root on the TOR VPS.' >&2
  exit 1
fi
# shellcheck disable=SC1091
. /etc/os-release
if [[ $ID != ubuntu || $VERSION_CODENAME != noble ]]; then
  echo 'Refusing: this script is for Ubuntu 24.04 (noble) only.' >&2
  exit 1
fi
available_kb=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
if [[ ! $available_kb =~ ^[0-9]+$ ]] || (( available_kb < 262144 )); then
  echo 'Refusing: less than 256 MiB free memory budget.' >&2
  exit 1
fi
if ss -lnt | grep -Eq '(^|[[:space:]])(0\.0\.0\.0|\[::\]|\*):5432([[:space:]]|$)'; then
  echo 'Refusing: an existing PostgreSQL server exposes a public TCP port.' >&2
  exit 1
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends ca-certificates curl postgresql-common
keydir=/usr/share/postgresql-common/pgdg
key=$keydir/apt.postgresql.org.asc
source=/etc/apt/sources.list.d/pgdg.sources
install -d -m 0755 "$keydir"
if [[ -L "$source" ]] || { [[ -e "$source" ]] && ! grep -Fq 'https://apt.postgresql.org/pub/repos/apt' "$source"; }; then
  echo 'Refusing to replace an unexpected PGDG apt source.' >&2
  exit 1
fi
curl --fail --silent --show-error --location --retry 3 --max-time 30 \
  https://www.postgresql.org/media/keys/ACCC4CF8.asc -o "$key"
chmod 0644 "$key"
cat > "$source" <<SOURCES
Types: deb
URIs: https://apt.postgresql.org/pub/repos/apt
Suites: noble-pgdg
Architectures: $(dpkg --print-architecture)
Components: main
Signed-By: $key
SOURCES
chmod 0644 "$source"
apt-get update
apt-get install -y --no-install-recommends postgresql-18

if ! pg_lsclusters -h | awk '$1 == "18" && $2 == "main" {found=1} END {exit !found}'; then
  pg_createcluster 18 main --start-conf=auto
fi
config=/etc/postgresql/18/main/postgresql.conf
if [[ ! -f "$config" ]]; then
  echo 'Missing PostgreSQL 18 main config.' >&2
  exit 1
fi
backup="$config.before-tor.$(date +%Y%m%d-%H%M%S)"
cp -a "$config" "$backup"

# For 1 vCPU and 961 MiB total RAM; benchmark before raising these.
pg_conftool 18 main set listen_addresses 127.0.0.1
pg_conftool 18 main set max_connections 20
pg_conftool 18 main set shared_buffers 64MB
pg_conftool 18 main set work_mem 2MB
pg_conftool 18 main set maintenance_work_mem 32MB
pg_conftool 18 main set autovacuum_work_mem 32MB
pg_conftool 18 main set autovacuum_max_workers 1
pg_conftool 18 main set max_parallel_workers 0
pg_conftool 18 main set max_parallel_workers_per_gather 0
pg_conftool 18 main set max_worker_processes 2
pg_conftool 18 main set effective_cache_size 256MB
pg_conftool 18 main set jit off

if ! systemctl restart postgresql@18-main.service; then
  echo 'PostgreSQL restart failed; restoring previous config.' >&2
  cp -a "$backup" "$config"
  systemctl restart postgresql@18-main.service || true
  exit 1
fi
systemctl enable postgresql.service
systemctl is-active --quiet postgresql@18-main.service
if ! runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -Atqc 'SELECT 1' | grep -qx 1; then
  echo 'Local PostgreSQL SQL probe failed.' >&2
  exit 1
fi
if ss -lnt | grep -Eq '(^|[[:space:]])(0\.0\.0\.0|\[::\]|\*):5432([[:space:]]|$)'; then
  echo 'Refusing exposed PostgreSQL TCP listener.' >&2
  exit 1
fi

echo '=== PostgreSQL and clusters ==='
psql --version
pg_lsclusters
echo '=== Settings ==='
runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -Atqc \
  "SELECT name || '=' || setting || COALESCE(unit,'') FROM pg_settings WHERE name IN ('listen_addresses','max_connections','shared_buffers','work_mem','autovacuum_max_workers') ORDER BY name"
echo '=== 5432 listener (must be 127.0.0.1 only) ==='
ss -lntp | grep ':5432' || true
echo '=== Resources ==='
free -h
df -h /
echo '=== TOR process remains active ==='
systemctl is-active tor-web.service
curl --fail --silent --show-error http://127.0.0.1:3000/readyz
printf '\nPostgreSQL bootstrap SUCCESS; no app database or roles created yet.\n'
