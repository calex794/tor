#!/usr/bin/env bash
# TOR I-05: initialise app DB and separately privileged migrator/app roles.
# Secrets are generated locally and are never sent to GitHub.
set -Eeuo pipefail
umask 077
[[ $(id -u) == 0 ]] || { echo 'Run as root' >&2; exit 1; }
systemctl is-active --quiet postgresql@18-main.service || { echo 'PG18 is not active' >&2; exit 1; }
version=$(runuser -u postgres -- psql -X -Atqc 'SHOW server_version_num')
[[ $version =~ ^18[0-9]{4}$ ]] || { echo "Expected PG18, got $version" >&2; exit 1; }
listen=$(runuser -u postgres -- psql -X -Atqc 'SHOW listen_addresses')
[[ $listen == 127.0.0.1 ]] || { echo "PG must listen only on 127.0.0.1" >&2; exit 1; }
[[ $(runuser -u postgres -- psql -X -Atqc 'SHOW password_encryption') == scram-sha-256 ]] || {
  echo 'Require scram-sha-256 authentication' >&2; exit 1;
}

dir=/etc/tor
[[ ! -L $dir ]] || { echo "Unexpected symlink: $dir" >&2; exit 1; }
install -d -m 0700 -o root -g root "$dir"
app_config="$dir/tor-app.env"
migrator_config="$dir/tor-migrator.env"

create_credential_file() {
  local role=$1 dest=$2 pass tmp
  [[ ! -L $dest ]] || { echo "Unexpected symlink: $dest" >&2; exit 1; }
  if [[ ! -e $dest ]]; then
    pass=$(openssl rand -hex 32)
    tmp=$(mktemp "$dir/.credentials.XXXXXXXX")
    printf 'DATABASE_URL=postgresql://%s:%s@127.0.0.1:5432/tor?sslmode=disable\n' \
      "$role" "$pass" > "$tmp"
    chmod 0600 "$tmp"
    mv -T "$tmp" "$dest"
  fi
  chmod 0600 "$dest"
  chown root:root "$dest"
}

get_password() {
  local role=$1 file=$2 uri re
  uri=$(cat "$file")
  re="^DATABASE_URL=postgresql://$role:([0-9a-f]{64})@127[.]0[.]0[.]1:5432/tor[?]sslmode=disable$"
  [[ $uri =~ $re ]] || { echo "Unexpected credential contents: $file" >&2; exit 1; }
  REPLY=$(printf '%s' "$uri" | cut -d: -f3 | cut -d@ -f1)
}

create_credential_file tor_app "$app_config"
create_credential_file tor_migrator "$migrator_config"
get_password tor_app "$app_config"; app_password=$REPLY
get_password tor_migrator "$migrator_config"; migrator_password=$REPLY

# Values are 64 lowercase hex characters; no SQL injection or process arguments.
runuser -u postgres -- psql -X -q -v ON_ERROR_STOP=1 <<SQL
DO \$new_roles\$
BEGIN
 IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
  CREATE ROLE tor_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION CONNECTION LIMIT 6;
 END IF;
 IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_migrator') THEN
  CREATE ROLE tor_migrator LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION CONNECTION LIMIT 3;
 END IF;
END
\$new_roles\$;
ALTER ROLE tor_app WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION CONNECTION LIMIT 6 PASSWORD '$app_password';
ALTER ROLE tor_migrator WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION CONNECTION LIMIT 3 PASSWORD '$migrator_password';
SQL

owner=$(runuser -u postgres -- psql -X -Atqc \
  "SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname='tor'")
[[ -z $owner || $owner == tor_migrator ]] || {
  echo "Database tor is already owned by $owner; refusing to alter it" >&2
  exit 1
}
if [[ -z $owner ]]; then
  runuser -u postgres -- createdb -O tor_migrator --encoding=UTF8 --template=template0 tor
fi

runuser -u postgres -- psql -X -q -v ON_ERROR_STOP=1 -d tor <<'SQL'
REVOKE ALL ON DATABASE tor FROM PUBLIC;
GRANT CONNECT ON DATABASE tor TO tor_app;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO tor_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO tor_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO tor_app;
ALTER DEFAULT PRIVILEGES FOR ROLE tor_migrator IN SCHEMA public
  GRANT USAGE, SELECT ON SEQUENCES TO tor_app;
SQL

# Check TCP/SCRAM authentication and rights without showing passwords.
PGHOST=127.0.0.1 PGPORT=5432 PGDATABASE=tor PGUSER=tor_app \
 PGPASSWORD="$app_password" psql -X -Atqc \
 "SELECT current_user || ':' || current_database() || ':' ||
    has_schema_privilege(current_user,'public','CREATE')" |
 grep -qx 'tor_app:tor:false'
PGHOST=127.0.0.1 PGPORT=5432 PGDATABASE=tor PGUSER=tor_migrator \
 PGPASSWORD="$migrator_password" psql -X -Atqc \
 "SELECT current_user || ':' || current_database() || ':' ||
    has_schema_privilege(current_user,'public','CREATE')" |
 grep -qx 'tor_migrator:tor:true'

echo '=== Role permissions (no secrets) ==='
runuser -u postgres -- psql -X -Atqc \
 "SELECT rolname,rolsuper,rolcreatedb,rolcreaterole,rolconnlimit
 FROM pg_roles WHERE rolname IN ('tor_app','tor_migrator') ORDER BY rolname"
echo '=== Services and memory ==='
pg_lsclusters
systemctl is-active tor-web.service
free -h
echo '=== Secret files (permissions only) ==='
stat -c '%U:%G %a %n' "$app_config" "$migrator_config"
echo 'SUCCESS: tor DB and restricted roles; Rust is still an isolated RAM demo.'
