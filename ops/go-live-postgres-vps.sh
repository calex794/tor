#!/usr/bin/env bash
# TOR: one-time go-live from S0 RAM demo to real PostgreSQL runtime.
# Run only on the dedicated TOR VPS as root with a verified staged main SHA.
set -Eeuo pipefail
umask 077

# TOR_RUNTIME_ENV_GUARD_BEGIN
# Accept previously documented combined app settings as well as the optional
# operator-installed runtime pair. Never source either file as shell or rewrite
# it. Stdout is private: the caller captures only the validated DB password.
validate_runtime_configuration() {
  local app_file="$1/tor-app.env" mode="${3:-release}"
  local runtime_file="$1/tor-runtime.env" runtime_dropin="$2/30-runtime.conf"
  [[ "$mode" == release || "$mode" == diagnostic ]] || return 2
  if [[ -L "$1" || $(stat -c '%U:%G:%a' "$1" 2>/dev/null) != root:root:700 ]]; then
    echo 'REFUSE_RUNTIME_ENV: unsafe secret directory /etc/tor' >&2
    return 1
  fi
  if [[ -L "$app_file" || ! -f "$app_file" ||
        $(stat -c '%U:%G:%a:%h' "$app_file" 2>/dev/null) != root:root:600:1 ]]; then
    echo 'REFUSE_RUNTIME_ENV: missing or unsafe tor-app.env' >&2
    return 1
  fi
  if [[ ! -e "$runtime_file" && ! -L "$runtime_file" &&
        ! -e "$runtime_dropin" && ! -L "$runtime_dropin" ]]; then
    runtime_file=""
  else
    if [[ -L "$2" || $(stat -c '%U:%G:%a' "$2" 2>/dev/null) != root:root:755 ]]; then
      echo 'REFUSE_RUNTIME_ENV: unsafe runtime drop-in directory' >&2
      return 1
    fi
    if [[ -L "$runtime_file" || ! -f "$runtime_file" ||
          $(stat -c '%U:%G:%a:%h' "$runtime_file" 2>/dev/null) != root:root:600:1 ]]; then
      echo 'REFUSE_RUNTIME_ENV: missing or unsafe tor-runtime.env' >&2
      return 1
    fi
    if [[ -L "$runtime_dropin" || ! -f "$runtime_dropin" ||
          $(stat -c '%U:%G:%a:%h' "$runtime_dropin" 2>/dev/null) != root:root:644:1 ]]; then
      echo 'REFUSE_RUNTIME_ENV: missing or unsafe 30-runtime.conf' >&2
      return 1
    fi
    if ! cmp -s "$runtime_dropin" <(printf '[Service]\nEnvironmentFile=/etc/tor/tor-runtime.env\n'); then
      echo 'REFUSE_RUNTIME_ENV: unexpected runtime systemd drop-in' >&2
      return 1
    fi
  fi
  # A deliberately small subset of systemd EnvironmentFile: unquoted ASCII
  # assignments and UTF-8 comments. Never relax credential/value validation.
  # Refuse unknown/duplicate names, continuation, expansion and raw diagnostics.
  if ! python3 -I - "$app_file" "$runtime_file" "$mode" <<'PY'
import base64
import binascii
import re
import sys
from pathlib import Path

flags = {
    "TOR_PAID_GRANTS_ENABLED", "TOR_PAID_AUTHOR_PUBLISH_ENABLED",
    "TOR_PAID_AUTHOR_S3_ONLY_ENABLED", "TOR_FREE_PACK_READER_ENABLED",
    "TOR_CHAPTER_IMPORT_PACK_DIRECT_ENABLED", "TOR_DROP_PACK_WORKER_ENABLED",
    "TOR_DROP_PACK_PLANNER_ENABLED", "TOR_DROP_PACK_CUTOVER_ENABLED",
    "TOR_COVER_WORKER_ENABLED",
}
buckets = {
    "TOR_FREE_S3_BUCKET", "TOR_CHAPTER_IMPORT_S3_BUCKET",
    "TOR_COVER_S3_BUCKET", "TOR_PAID_S3_BUCKET",
}
origins = {"TOR_FREE_S3_PUBLIC_ORIGIN", "TOR_PAID_S3_PUBLIC_ORIGIN"}
keyring = {"TOR_CONTENT_KEK_V1", "TOR_CONTENT_KEK_V2"}
allowed = flags | buckets | origins | keyring | {
    "AWS_REGION", "AWS_ENDPOINT_URL_S3", "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY", "TOR_S3_ADMIN_MASTER_KEY",
    "TOR_CONTENT_ACTIVE_KEK_ID",
}

# Report only fixed filenames, line numbers, reviewed variable names and
# static reason codes. In particular, an unknown identifier could itself be
# a secret, so never echo it (or a raw line, value or exception message).
known_names = allowed | {
    "DATABASE_URL", "TOR_AUTH_ENABLED", "TOR_DEMO_MODE",
    "TOR_LISTEN_ADDR", "LD_PRELOAD",
}
problems = []


def check(ok, reason, file, line=0, name=None):
    if not ok:
        safe_name = name if name in known_names else "unknown"
        problems.append(
            f"TOR_RUNTIME_ENV_DIAG file={file} line={line} "
            f"parameter={safe_name} reason={reason}"
        )
    return bool(ok)


db_url = re.compile(
    r"postgresql://tor_app:([0-9a-f]{64})@127[.]0[.]0[.]1:5432/tor[?]sslmode=disable"
)


def read_env(path, *, app):
    file = "tor-app.env" if app else "tor-runtime.env"
    values, locations, seen = {}, {}, set()
    try:
        with Path(path).open("rb") as stream:
            raw = stream.read(65537)
    except OSError:
        check(False, "unreadable-file", file)
        return values, locations
    if not check(0 < len(raw) <= 65536, "empty-or-oversized-file", file):
        return values, locations
    # Preserve the old DB-only file's acceptance with or without final LF.
    check(app or raw.endswith(b"\n"), "missing-final-lf", file)
    try:
        lines = raw.decode("utf-8").split("\n")
    except UnicodeDecodeError as exc:
        check(False, "invalid-utf8-file", file, raw.count(b"\n", 0, exc.start) + 1)
        return values, locations
    for number, line in enumerate(lines, 1):
        if not line.strip(" ") or line.lstrip(" ").startswith("#"):
            check(not re.search(r"[\x00-\x1f\x7f]", line),
                  "control-character", file, number)
            continue
        match = re.fullmatch(r"([A-Z][A-Z0-9_]*)=(.*)", line)
        if not check(match is not None, "invalid-assignment", file, number):
            continue
        name, value = match.groups()
        if not check(name not in seen, "duplicate-parameter", file, number, name):
            continue
        seen.add(name)
        if app and name == "DATABASE_URL":
            if not check(db_url.fullmatch(value), "invalid-local-database-url",
                         file, number, name):
                continue
        else:
            if not check(name in allowed, "unsupported-parameter", file, number, name):
                continue
            if not check(re.fullmatch(r"[A-Za-z0-9+/=_:.-]{1,2048}", value),
                         "invalid-value-syntax", file, number, name):
                continue
        values[name] = value
        locations[name] = (file, number)
    check(bool(values), "no-valid-assignments", file)
    return values, locations


def validate(values, locations):
    def setting(ok, name, reason):
        file, line = locations[name]
        return check(ok, reason, file, line, name)

    for name, value in values.items():
        if name in flags:
            setting(value in {"0", "1"}, name, "expected-boolean-01")
        elif name in buckets:
            setting(re.fullmatch(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]", value)
                    and ".." not in value, name, "invalid-bucket-name")
        elif name in origins:
            setting(re.fullmatch(r"https://[a-zA-Z0-9](?:[a-zA-Z0-9.-]*[a-zA-Z0-9])?", value)
                    and ".." not in value, name, "invalid-https-origin")
        elif name == "AWS_REGION":
            setting(re.fullmatch(r"[a-z0-9-]{2,64}", value), name, "invalid-region")
        elif name == "AWS_ENDPOINT_URL_S3":
            setting(value == "https://storage.yandexcloud.net", name, "unsupported-s3-endpoint")
        elif name == "AWS_ACCESS_KEY_ID":
            setting(8 <= len(value) <= 128, name, "invalid-access-key-length")
        elif name == "AWS_SECRET_ACCESS_KEY":
            setting(16 <= len(value) <= 256, name, "invalid-secret-key-length")
        elif name == "TOR_S3_ADMIN_MASTER_KEY" or name in keyring:
            try:
                if name == "TOR_S3_ADMIN_MASTER_KEY":
                    key = base64.b64decode(value, validate=True)
                    canonical = base64.b64encode(key).decode()
                else:
                    key = base64.b64decode(value + "=", altchars=b"-_", validate=True)
                    canonical = base64.urlsafe_b64encode(key).decode().rstrip("=")
                valid = len(key) == 32 and canonical == value
            except (ValueError, binascii.Error):
                valid = False
            setting(valid, name, "invalid-key-encoding")
        elif name == "TOR_CONTENT_ACTIVE_KEK_ID":
            setting(value in {"content-kek-v1", "content-kek-v2"},
                    name, "invalid-active-kek-id")
    if ("AWS_ACCESS_KEY_ID" in values) != ("AWS_SECRET_ACCESS_KEY" in values):
        name = "AWS_ACCESS_KEY_ID" if "AWS_ACCESS_KEY_ID" in values else "AWS_SECRET_ACCESS_KEY"
        setting(False, name, "incomplete-aws-credential-pair")
    if "TOR_CONTENT_KEK_V2" in values:
        setting("TOR_CONTENT_KEK_V1" in values, "TOR_CONTENT_KEK_V2", "missing-kek-v1")
        if "TOR_CONTENT_KEK_V1" in values:
            setting(values["TOR_CONTENT_KEK_V1"] != values["TOR_CONTENT_KEK_V2"],
                    "TOR_CONTENT_KEK_V2", "duplicate-kek-material")
    if values.get("TOR_CONTENT_ACTIVE_KEK_ID") in {"content-kek-v1", "content-kek-v2"}:
        required = ("TOR_CONTENT_KEK_V2" if values["TOR_CONTENT_ACTIVE_KEK_ID"] == "content-kek-v2"
                    else "TOR_CONTENT_KEK_V1")
        setting(required in values, "TOR_CONTENT_ACTIVE_KEK_ID", "missing-active-kek")
    if values.get("TOR_PAID_GRANTS_ENABLED") == "1":
        setting("TOR_CONTENT_KEK_V1" in values, "TOR_PAID_GRANTS_ENABLED", "missing-kek-v1")


try:
    values, locations = read_env(sys.argv[1], app=True)
    check("DATABASE_URL" in values, "missing-valid-database-url", "tor-app.env",
          name="DATABASE_URL")
    password = db_url.fullmatch(values.pop("DATABASE_URL")).group(1) if "DATABASE_URL" in values else None
    if sys.argv[2]:
        separate, other_locations = read_env(sys.argv[2], app=False)
        for name in sorted(values.keys() & separate.keys()):
            file, line = other_locations[name]
            check(False, "duplicate-across-files", file, line, name)
        for name, value in separate.items():
            if name not in values:
                values[name] = value
                locations[name] = other_locations[name]
    validate(values, locations)
    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        sys.exit(1)
    if sys.argv[3] == "diagnostic":
        print("TOR_RUNTIME_ENV_OK")
    else:
        print(password)
except Exception:
    # No exception trace or text can be allowed to leak secret material.
    print("TOR_RUNTIME_ENV_DIAG reason=internal-validation-error", file=sys.stderr)
    sys.exit(1)
PY
  then
    echo 'REFUSE_RUNTIME_ENV: invalid or unsupported runtime settings' >&2
    return 1
  fi
}
# TOR_RUNTIME_ENV_GUARD_END

# TOR_RUNTIME_ENV_DIAGNOSTIC_BEGIN
# Read-only operator command: no candidate, SQL, network or service actions.
if [[ $# == 1 && $1 == --check-runtime ]]; then
  if [[ $(id -u) -ne 0 ]]; then
    echo "REFUSE_RUNTIME_ENV: root privileges required" >&2
    exit 2
  fi
  validate_runtime_configuration /etc/tor /etc/systemd/system/tor-web.service.d diagnostic
  exit 0
fi
# TOR_RUNTIME_ENV_DIAGNOSTIC_END

sha=${1:-}
recovery=0
if [[ $# == 2 && $2 == --recover ]]; then
  recovery=1
elif [[ $# != 1 ]]; then
  echo "Usage: root bash go-live-postgres-vps.sh <STAGED_MAIN_SHA> [--recover] | --check-runtime" >&2
  exit 2
fi
if [[ $(id -u) -ne 0 || ! $sha =~ ^[0-9a-f]{40}$ ]]; then
  echo "Refusing: root and a full SHA required" >&2
  exit 2
fi

# TOR_MAIN_SHA_GUARD_BEGIN
# Fail closed before any SQLx migration when the selected staged SHA is stale.
require_current_main_sha() {
  local actual_main_sha
  if ! command -v python3 >/dev/null 2>&1; then
    echo "REFUSE_OBSOLETE_MAIN: python3 is required to verify GitHub main" >&2
    return 1
  fi
  actual_main_sha=$(
    curl --fail --silent --show-error --location --retry 2 --max-time 15 \
      -H "Accept: application/vnd.github+json" \
      https://api.github.com/repos/calex794/tor/git/ref/heads/main |
      python3 -c '
import json
import sys
payload = json.load(sys.stdin)
if payload.get("ref") != "refs/heads/main":
    sys.exit(1)
print(payload.get("object", {}).get("sha", ""))
'
  ) || {
    echo "REFUSE_OBSOLETE_MAIN: could not verify GitHub main; no release permitted" >&2
    return 1
  }
  if [[ ! "$actual_main_sha" =~ ^[0-9a-f]{40}$ || "$actual_main_sha" != "$sha" ]]; then
    echo "REFUSE_OBSOLETE_MAIN: staged SHA differs from verified GitHub main" >&2
    return 1
  fi
}
# TOR_MAIN_SHA_GUARD_END
require_current_main_sha

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
if [[ "$recovery" == 1 ]]; then
  [[ $(cat /var/lib/tor-release/pg-activated) == PG_ACTIVATED ]] || {
    echo "Refusing recovery without verified previous PostgreSQL activation" >&2
    exit 1
  }
  if systemctl is-active --quiet tor-web.service &&
     [[ $(curl -fsS --max-time 5 http://127.0.0.1:3000/readyz 2>/dev/null || true) == ok ]]; then
    echo "Refusing recovery: existing website is healthy" >&2
    exit 1
  fi
else
  systemctl is-active --quiet tor-web.service
fi

# Verify the actual staged executable's compiled profile before any SQLx
# migration, not just the workflow command or a string in the manifest.
# Run without credentials and without root privileges.
expected_profile='TOR_RELEASE_CAPABILITIES s3-covers=true s3-free=true s3-paid=true s3-chapter-import=true'
actual_profile=$(runuser -u tor-deploy -- env -i PATH=/usr/bin:/bin \
  timeout 10s "$release/tor-web" --release-capabilities 2>/dev/null) || {
  echo "REFUSE_RELEASE_PROFILE: cannot read staged binary capabilities" >&2
  exit 1
}
[[ "$actual_profile" == "$expected_profile" ]] || {
  echo "REFUSE_RELEASE_PROFILE: incomplete production S3 capabilities" >&2
  exit 1
}
printf '%s\n' "$actual_profile"

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

# Fail before migrations if either configuration is unsafe. Capture the
# password privately and still prove a real local TCP login as tor_app.
app_password=$(validate_runtime_configuration "$secret_dir" /etc/systemd/system/tor-web.service.d)
app_whoami=$(PGHOST=127.0.0.1 PGPORT=5432 PGSSLMODE=disable PGUSER=tor_app PGDATABASE=tor \
 PGPASSWORD="$app_password" psql -X -w -v ON_ERROR_STOP=1 -Atqc 'SELECT current_user' 2>/dev/null)
unset app_password
if [[ $app_whoami != tor_app ]]; then
  echo "Application role TCP/SCRAM authentication failed" >&2
  exit 1
fi

# Apply the canonical SQLx migrator from the verified staged release.
# It creates the initial schema or upgrades additive migrations in place.
migration_script=$(mktemp /root/.tor-migration.XXXXXXXX)
trap 'rm -f "$migration_script"' EXIT
curl --fail --silent --show-error --location --retry 3 \
  "https://raw.githubusercontent.com/calex794/tor/$sha/ops/apply-tor-migrations-vps.sh" \
  -o "$migration_script"
bash -n "$migration_script"
if [[ "$recovery" == 1 ]]; then
  # Stop the known-broken service's restart loop only after validation.
  systemctl stop tor-web.service
  bash "$migration_script" "$sha" --recover
else
  bash "$migration_script" "$sha"
fi
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

# Check again: main may have changed while SQLx ran. Old binary remains active
# on refusal; an already applied SQLx migration must be handled separately.
require_current_main_sha

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
  systemctl daemon-reload || true
  if [[ "$recovery" == 1 ]]; then
    # The previous binary was already broken at entry. Never restart it
    # indefinitely or advertise a restored service during recovery.
    systemctl stop tor-web.service || true
    echo "ROLLBACK_UNVERIFIED: previous binary pointer restored; recovery failed, service stopped; SQLx migrations are preserved." >&2
    return 0
  fi
  systemctl restart tor-web.service || true
  # A restored symlink alone is NOT proof that the previous binary works with
  # the new additive SQLx schema. Never claim rollback success without HTTP.
  rollback_healthy=0
  for attempt in $(seq 1 15); do
    if systemctl is-active --quiet tor-web.service &&
       [[ $(curl -fsS --max-time 3 http://127.0.0.1:3000/readyz 2>/dev/null || true) == ok ]]; then
      rollback_healthy=1
      break
    fi
    sleep 1
  done
  if [[ "$rollback_healthy" == 1 ]] &&
     [[ $(curl -fsS --max-time 6 --resolve pandaolate.ru:443:127.0.0.1 https://pandaolate.ru/readyz 2>/dev/null || true) == ok ]]; then
    echo "ROLLBACK_VERIFIED: previous TOR binary healthy; SQLx migrations are preserved." >&2
  else
    echo "ROLLBACK_UNVERIFIED: previous binary restored but real HTTPS readiness failed; SQLx migrations are preserved." >&2
    systemctl show tor-web.service --no-pager -p ActiveState -p SubState -p Result -p ExecMainStatus || true
    if [[ "$rollback_healthy" != 1 ]]; then
      systemctl stop tor-web.service || true
      echo "TOR_ROLLBACK_SERVICE_STOPPED: manual recovery required" >&2
    fi
  fi
}
trap undo ERR

# Retain the existing PostgreSQL drop-in. The separate runtime pair is
# operator-owned, never rewritten, generated or removed by a release.
# Recheck it before switching (operator changes must not race a release).
validate_runtime_configuration "$secret_dir" "$dropin_dir" >/dev/null
# Validate mandatory security invariants and retain the operator's settings.
if [[ -f "$dropin" ]]; then
  for line in \
    'Environment=TOR_DEMO_MODE=0' \
    'Environment=TOR_AUTH_ENABLED=1' \
    'Environment=TOR_PUBLIC_ORIGIN=https://pandaolate.ru' \
    'EnvironmentFile=/etc/tor/tor-app.env'; do
    grep -Fxq "$line" "$dropin" || {
      echo "Refusing unsafe production systemd override" >&2
      false
    }
  done
else
  # First-time PostgreSQL activation only; PIN 1 reads the root-only secret.
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
fi


tmp="/opt/tor/.go-live-$$"
ln -s "$release" "$tmp"
mv -Tf "$tmp" /opt/tor/current
switched=1
systemctl daemon-reload
if [[ "$recovery" == 1 ]]; then
  systemctl reset-failed tor-web.service
fi
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

# The schema marker only proves that SQLx ran. Auto-activation must also
# have evidence that the real PostgreSQL runtime completed HTTPS acceptance.
# Never write this root-owned marker before successful production cutover.
pg_active_marker="$schema_dir/pg-activated"
if [[ -L $pg_active_marker ]]; then
  echo "Refusing symlink for PostgreSQL activation marker" >&2
  false
fi
pg_active_tmp=$(mktemp "$schema_dir/.pg-activated.XXXXXXXX")
printf '%s\n' 'PG_ACTIVATED' > "$pg_active_tmp"
chown root:root "$pg_active_tmp"
chmod 0644 "$pg_active_tmp"
mv -fT "$pg_active_tmp" "$pg_active_marker"

trap - ERR
[[ -z $backup_dropin ]] || rm -f "$backup_dropin"
echo "TOR_PRODUCTION_OK $sha"
echo "POSTGRES_READY_OK ($hist/$hist migrations, tor_app read-only schema history)"
echo "AUTH_PAGES_OK /register /login"
echo "HTTPS_READY_OK https://pandaolate.ru/readyz"
echo "REAL_DATA_MODE: PostgreSQL; no RAM demo"
free -h
