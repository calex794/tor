#!/usr/bin/env bash
# TOR: one-time go-live from S0 RAM demo to real PostgreSQL runtime.
# Run only on the dedicated TOR VPS as root with a verified staged main SHA.
set -Eeuo pipefail
umask 077

sha=${1:-}
if [[ $(id -u) -ne 0 || $# -ne 1 || ! $sha =~ ^[0-9a-f]{40}$ ]]; then
  echo "Usage: root bash go-live-postgres-vps.sh <STAGED_MAIN_SHA>" >&2
  exit 2
fi

release="/opt/tor/releases/$sha"
if [[ ! -x $release/tor-web || ! -x $release/tor-migrate ||
      -L $release || $(readlink -f "$release") != "$release" ||
      ! -f $release/tor-migrate.sha256 ]]; then
  echo "TOR release $sha was not staged with both Rust binaries" >&2
  exit 1
fi
(cd "$release" && sha256sum -c --status tor-migrate.sha256) || {
  echo "SQLx migration binary checksum mismatch" >&2
  exit 1
}
systemctl is-active --quiet postgresql@18-main.service
systemctl is-active --quiet tor-web.service

secret_dir=/etc/tor
app_secret=$secret_dir/tor-app.env
migrator_secret=$secret_dir/tor-migrator.env
if [[ $(stat -c '%U:%G:%a' "$secret_dir") != root:root:700 ]]; then
  echo "Refusing unexpected TOR secret directory permissions" >&2
  exit 1
fi
for secret in "$app_secret" "$migrator_secret"; do
  if [[ -L $secret || ! -f $secret ||
        $(stat -c '%U:%G:%a' "$secret") != root:root:600 ]]; then
    echo "Refusing unsafe file permissions for $secret" >&2
    exit 1
  fi
done

# Validate app authentication without sending the password via CLI arguments.
app_line=$(cat "$app_secret")
app_pattern='^DATABASE_URL=postgresql://tor_app:([0-9a-f]{64})@127[.]0[.]0[.]1:5432/tor[?]sslmode=disable$'
if [[ ! $app_line =~ $app_pattern ]]; then
  echo "Refusing: invalid tor_app DATABASE_URL" >&2
  exit 1
fi
app_password=${BASH_REMATCH[1]}
app_whoami=$(PGHOST=127.0.0.1 PGUSER=tor_app PGDATABASE=tor \
 PGPASSWORD="$app_password" psql -X -v ON_ERROR_STOP=1 -Atqc 'SELECT current_user' 2>/dev/null)
if [[ $app_whoami != tor_app ]]; then
  echo "Application role TCP/SCRAM authentication failed" >&2
  exit 1
fi
unset app_password app_line

# Apply the canonical SQLx migrator from the verified staged release.
# It creates the initial schema or upgrades additive migrations in place.
migration_script=$(mktemp /root/.tor-migration.XXXXXXXX)
trap 'rm -f "$migration_script"' EXIT
curl --fail --silent --show-error --location --retry 3 \
  "https://raw.githubusercontent.com/calex794/tor/$sha/ops/apply-tor-migrations-vps.sh" \
  -o "$migration_script"
bash -n "$migration_script"
bash "$migration_script" "$sha"
rm -f "$migration_script"
trap - EXIT

hist=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations WHERE success = TRUE")
hist_total=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations")
donor_hist=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT count(*) FROM _sqlx_migrations WHERE success = TRUE
   AND version IN (202610120001,202610120002,202610120003,202610120004,
                   202610120005,202610120006,202610130001,202610130002,
                   202610130003)")
if [[ ! $hist =~ ^[0-9]+$ || $hist -lt 9 ||
      $hist_total != "$hist" || $donor_hist != 9 ]]; then
  echo "Refusing: SQLx history is incomplete or dirty" >&2
  exit 1
fi
roles_ok=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT has_table_privilege('tor_app','public._sqlx_migrations','SELECT')::text
       || ',' || has_table_privilege('tor_app','public._sqlx_migrations','INSERT')::text
       || ',' || has_table_privilege('tor_app','public._sqlx_migrations','UPDATE')::text
       || ',' || has_table_privilege('tor_app','public._sqlx_migrations','DELETE')::text")
if [[ $roles_ok != true,false,false,false ]]; then
  echo "Refusing: runtime user lacks read-only SQLx history permission" >&2
  exit 1
fi

# Root-owned release marker records verified database schema, not code state.
# The restricted tor-deploy SSH user can READ it but cannot forge a version.
latest_schema=$(runuser -u postgres -- psql -X -v ON_ERROR_STOP=1 -d tor -Atqc \
  "SELECT max(version) FROM _sqlx_migrations WHERE success=TRUE")
if [[ ! $latest_schema =~ ^[0-9]{12}$ ]]; then
  echo "Refusing: could not verify last successful SQLx schema version" >&2
  exit 1
fi
schema_dir=/var/lib/tor-release
schema_marker=$schema_dir/schema-version
if [[ -L $schema_dir || -L $schema_marker ]]; then
  echo "Refusing symlink at schema marker location" >&2
  exit 1
fi
install -d -o root -g root -m 0755 "$schema_dir"
schema_tmp=$(mktemp "$schema_dir/.schema-version.XXXXXXXX")
printf '%s\n' "$latest_schema" > "$schema_tmp"
chown root:root "$schema_tmp"
chmod 0644 "$schema_tmp"
mv -fT "$schema_tmp" "$schema_marker"
echo "TOR_SCHEMA_MARKED $latest_schema"

dropin_dir=/etc/systemd/system/tor-web.service.d
dropin="$dropin_dir/20-postgres.conf"
install -d -m 0755 -o root -g root "$dropin_dir"
[[ ! -L $dropin ]] || { echo "Unsafe systemd override symlink" >&2; exit 1; }

old_target=$(readlink /opt/tor/current || true)
if [[ -z $old_target || ! -x "$old_target/tor-web" ]]; then
  echo "Could not identify previously working TOR web executable" >&2
  exit 1
fi
backup_dropin=""
if [[ -e $dropin ]]; then
  backup_dropin=$(mktemp /root/.tor-dropin-backup.XXXXXXXX)
  cp -a "$dropin" "$backup_dropin"
fi

switched=0
undo() {
  trap - ERR
  echo "Production cutover failed. Returning to prior service configuration." >&2
  if [[ -n $backup_dropin ]]; then
    cp -a "$backup_dropin" "$dropin"
  else
    rm -f "$dropin"
  fi
  if [[ $switched == 1 ]]; then
    back="/opt/tor/.rollback-$$"
    ln -s "$old_target" "$back"
    mv -Tf "$back" /opt/tor/current
  fi
  systemctl daemon-reload
  systemctl restart tor-web.service || true
  echo "Restored previous TOR service settings; PostgreSQL migrations are preserved." >&2
}
trap undo ERR

# One-time systemd override: same loopback port, but *real* database and auth.
# PID 1 loads root-only EnvironmentFile before dropping privileges to tor-deploy.
cat > "$dropin" <<'SYSTEMD'
[Unit]
Description=TOR Rust + PostgreSQL website
After=postgresql@18-main.service
Wants=postgresql@18-main.service

[Service]
Environment=TOR_DEMO_MODE=0
Environment=TOR_AUTH_ENABLED=1
Environment=TOR_PUBLIC_ORIGIN=https://pandaolate.ru
EnvironmentFile=/etc/tor/tor-app.env
SYSTEMD
chmod 0644 "$dropin"


tmp="/opt/tor/.go-live-$$"
ln -s "$release" "$tmp"
mv -Tf "$tmp" /opt/tor/current
switched=1
systemctl daemon-reload
systemctl restart tor-web.service

ok=0
for attempt in $(seq 1 30); do
  if curl -fsS --max-time 2 http://127.0.0.1:3000/readyz 2>/dev/null | grep -qx ok; then
    ok=1
    break
  fi
  sleep 1
done
[[ $ok == 1 ]] || { echo "Database-backed /readyz failed" >&2; false; }
curl -fsS --max-time 6 http://127.0.0.1:3000/catalog >/dev/null
register_html=$(curl -fsS --max-time 6 http://127.0.0.1:3000/register)
login_html=$(curl -fsS --max-time 6 http://127.0.0.1:3000/login)
[[ $register_html == *"Регистрац"* && $register_html == *'name="csrf_token"'* ]]
[[ $login_html == *"Вход"* && $login_html == *'name="csrf_token"'* ]]
books_payload=$(curl -fsS --max-time 6 http://127.0.0.1:3000/api/v1/books)
if [[ $books_payload == *"Первое знакомство"* ]]; then
  echo "Refusing: old hard-coded demo book appears in real runtime API" >&2
  false
fi

# HTTPS still routes to the same loopback service; check externally via local Nginx.
curl -fsS --max-time 8 --resolve pandaolate.ru:443:127.0.0.1 \
  https://pandaolate.ru/readyz | grep -qx ok

trap - ERR
[[ -z $backup_dropin ]] || rm -f "$backup_dropin"
echo "TOR_PRODUCTION_OK $sha"
echo "POSTGRES_READY_OK ($hist/$hist migrations, tor_app read-only schema history)"
echo "AUTH_PAGES_OK /register /login"
echo "HTTPS_READY_OK https://pandaolate.ru/readyz"
echo "REAL_DATA_MODE: PostgreSQL; no RAM demo"
free -h
