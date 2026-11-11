#!/usr/bin/env bash
# I-11: destructive recovery ONLY inside a disposable, mount-free container.
set -Eeuo pipefail
umask 077
export LC_ALL=C
[[ ${TOR_I11_DISPOSABLE_CONTAINER:-} == yes && -f /.dockerenv ]] || exit 2
[[ $(id -u) != 0 ]] || exit 2
export PATH="/usr/lib/postgresql/18/bin:$PATH"
work=$(mktemp -d /tmp/tor-i11-pitr.XXXXXXXX)
source_dir="$work/source"
restore_dir="$work/restored"
mkdir -p "$work"/{repo,log,lock,spool,source-socket,restore-socket,bad-socket}
pg_version=$(postgres --version)
backrest_version=$(pgbackrest version)
[[ $pg_version == *' 18.'* ]] || exit 2
key=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
conf="$work/pgbackrest.conf"
cat > "$conf" <<CONFIG
[global]
repo1-type=posix
repo1-path=$work/repo
repo1-cipher-type=aes-256-cbc
repo1-cipher-pass=$key
log-level-console=warn
log-path=$work/log
lock-path=$work/lock
spool-path=$work/spool
start-fast=y
process-max=2
archive-timeout=30
[tor]
pg1-path=$source_dir
pg1-port=55432
pg1-socket-path=$work/source-socket
pg1-user=postgres
CONFIG
cleanup() {
  local status=$?
  trap - EXIT
  for dir in "$source_dir" "$restore_dir" "$work/bad-target"; do
    if [[ -f "$dir/postmaster.pid" ]]; then
      pg_ctl -D "$dir" -m immediate -w -t 10 stop >/dev/null 2>&1 || status=1
    fi
  done
  rm -rf -- "$work"
  exit "$status"
}
report_failure() {
  echo "I11_PITR_FAILED line=$1" >&2
  # Publish only a bounded redacted pgBackRest error, never PG logs/config/keys.
  python3 - "$work" "$key" <<'PY' >&2
from pathlib import Path
import re, sys
root, key = sys.argv[1:]
for name in ('wrong-key', 'restore', 'bad-restore'):
    path = Path(root) / 'log' / (name + '.log')
    if not path.is_file():
        continue
    for line in path.read_text(errors='replace').splitlines():
        match = re.search(r'ERROR:\s*\[([0-9]{3})\]:?\s*(.*)', line)
        if match:
            text = match[2].replace(key, '[redacted]').replace(root, '[temp]')
            text = re.sub(r'[^\x20-\x7e]', '?', text)[:360]
            print(f'I11_PITR_DIAGNOSTIC {name} {match[1]} {text}')
            break
PY
}
# pg_ctl -w accepts PM_STATUS_STANDBY as well as READY. Neither its return
# code nor an open socket proves that WAL replay has finished and SQL works.
wait_for_primary() {
  local deadline=$((SECONDS + ${1:-30})) state
  while ((SECONDS < deadline)); do
    if ! pg_ctl -D "$restore_dir" status >/dev/null 2>&1; then
      echo 'FAIL: restored PostgreSQL exited before primary readiness' >&2
      return 1
    fi
    if state=$(PGCONNECT_TIMEOUT=2 PGOPTIONS='-c statement_timeout=2000' \
        restored_sql -c 'SELECT pg_is_in_recovery()' 2>"$work/log/readiness-client.log"); then
      case "$state" in
        f) return 0 ;;
        t) ;; # A readable standby is still not the requested promoted database.
        *) echo 'FAIL: unexpected restored recovery state' >&2; return 1 ;;
      esac
    fi
    sleep 0.2
  done
  echo 'FAIL: restored PostgreSQL did not become a SQL-ready primary' >&2
  return 1
}

wait_for_target_failure() {
  local deadline=$((SECONDS + ${1:-30}))
  while pg_ctl -D "$work/bad-target" status >/dev/null 2>&1; do
    if ((SECONDS >= deadline)); then
      echo 'FAIL: unreachable recovery target stayed running' >&2
      return 1
    fi
    sleep 0.2
  done
  # An arbitrary crash is not proof of correct recovery-target enforcement.
  grep -Fq 'recovery ended before configured recovery target was reached' "$work/log/bad-start.log"
}

trap cleanup EXIT
trap 'report_failure "$LINENO"' ERR
trap 'exit 130' INT
trap 'exit 143' TERM
initdb -D "$source_dir" -U postgres --auth=trust >"$work/log/initdb.log"
cat >> "$source_dir/postgresql.conf" <<CONFIG
listen_addresses = ''
port = 55432
unix_socket_directories = '$work/source-socket'
unix_socket_permissions = 0700
wal_level = replica
archive_mode = on
archive_command = 'pgbackrest --config=$conf --stanza=tor archive-push %p'
archive_timeout = 900
max_connections = 16
shared_buffers = '32MB'
fsync = on
synchronous_commit = on
CONFIG
pg_ctl -D "$source_dir" -l "$work/log/source.log" -w -t 30 start >/dev/null
sql() { psql -X -v ON_ERROR_STOP=1 -At -h "$work/source-socket" -p 55432 -U postgres -d postgres "$@"; }
backrest() { pgbackrest --config="$conf" --stanza=tor "$@"; }
backrest stanza-create >"$work/log/stanza.log" 2>&1
backrest check >"$work/log/check.log" 2>&1
sql <<'SQL' >/dev/null
CREATE ROLE i11_reader NOLOGIN;
CREATE TABLE public.i11_recovery_markers (id text PRIMARY KEY);
REVOKE ALL ON TABLE public.i11_recovery_markers FROM PUBLIC;
INSERT INTO public.i11_recovery_markers VALUES ('base_snapshot');
SQL
backrest --type=full backup >"$work/log/backup.log" 2>&1
backrest --output=json info >"$work/info.json"
label=$(python3 - "$work/info.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding='utf-8') as stream:
    info = json.load(stream)
stanzas = [row for row in info if row['name'] == 'tor']
assert len(stanzas) == 1 and len(stanzas[0]['backup']) == 1
backup = stanzas[0]['backup'][0]
assert backup['type'] == 'full'
print(backup['label'])
PY
)
# This marker is AFTER the base backup: only WAL replay can restore it.
sql -c "INSERT INTO public.i11_recovery_markers VALUES ('before_target');" >/dev/null
sql -c "SELECT pg_create_restore_point('tor_i11_target');" >/dev/null
sql -c "INSERT INTO public.i11_recovery_markers VALUES ('after_target');" >/dev/null
[[ $(sql -c 'SELECT count(*) FROM public.i11_recovery_markers') == 3 ]]
sql -c 'SELECT pg_switch_wal();' >/dev/null
backrest check >"$work/log/final-archive-check.log" 2>&1
started=$(python3 -c 'import time; print(time.monotonic())')
pg_ctl -D "$source_dir" -m immediate -w -t 30 stop >/dev/null
rm -rf -- "$source_dir"
[[ ! -e "$source_dir" ]]
# Use identical destination/set/target for negative and positive controls.
# pgBackRest may wrap cipher failures as BackupSetInvalidError; a generic
# failure is NOT evidence unless the otherwise-identical correct-key run works.
restore_target() {
  pgbackrest --config="$1" --stanza=tor --pg1-path="$restore_dir" \
    --set="$label" --type=name --target=tor_i11_target --target-action=promote restore
}
sed "s/^repo1-cipher-pass=.*/repo1-cipher-pass=deliberately-wrong-test-key/" "$conf" >"$work/wrong.conf"
if restore_target "$work/wrong.conf" >"$work/log/wrong-key.log" 2>&1; then
  echo 'FAIL: wrong encryption key accepted' >&2
  exit 1
fi
[[ ! -e "$restore_dir/global/pg_control" ]]
rm -rf -- "$restore_dir"
restore_target "$conf" >"$work/log/restore.log" 2>&1
pg_ctl -D "$restore_dir" -l "$work/log/restored.log" -w -t 30 \
  -o "-c listen_addresses='' -c port=55433 -c unix_socket_directories=$work/restore-socket -c archive_mode=off -c hot_standby=off" start >/dev/null
restored_sql() { psql -X -v ON_ERROR_STOP=1 -At -h "$work/restore-socket" -p 55433 -U postgres -d postgres "$@"; }
wait_for_primary 30
[[ $(restored_sql -c 'SELECT pg_is_in_recovery()') == f ]]
[[ $(restored_sql -c "SELECT string_agg(id,',' ORDER BY id) FROM public.i11_recovery_markers") == 'base_snapshot,before_target' ]]
if restored_sql -c 'SET ROLE i11_reader; SELECT * FROM public.i11_recovery_markers' >"$work/log/acl.log" 2>&1; then
  echo 'FAIL: restored role acquired forbidden table access' >&2
  exit 1
fi
grep -q 'permission denied for table i11_recovery_markers' "$work/log/acl.log"
# Keep hot standby off, then observe the eventual process exit and exact fatal.
# pg_ctl may return success in STANDBY before the unreachable target is detected.
backrest --pg1-path="$work/bad-target" --set="$label" --type=name \
  --target=tor_i11_nonexistent --target-action=promote restore >"$work/log/bad-restore.log" 2>&1
bad_start_status=0
pg_ctl -D "$work/bad-target" -l "$work/log/bad-start.log" -w -t 30 \
    -o "-c listen_addresses='' -c port=55434 -c unix_socket_directories=$work/bad-socket -c archive_mode=off -c hot_standby=off" start >"$work/log/bad-pgctl.log" 2>&1 || bad_start_status=$?
# Both early startup success and immediate failure require the same terminal proof.
wait_for_target_failure 30
python3 - "$started" "$pg_version" "$backrest_version" "$label" <<'PY'
import json, sys, time
print(json.dumps({
    'schema_version': 1, 'scope': 'isolated_container_fixture',
    'postgres_version': sys.argv[2], 'pgbackrest_version': sys.argv[3],
    'backup_label': sys.argv[4], 'source_removed': True,
    'base_marker_restored': True, 'post_backup_wal_marker_restored': True,
    'post_target_marker_absent': True, 'wrong_key_rejected': True,
    'unreachable_target_rejected': True, 'role_denial_preserved': True,
    'recovery_promoted': True,
    'drill_elapsed_seconds': round(time.monotonic() - float(sys.argv[1]), 3),
    'production_tested': False, 'offsite_tested': False,
    'application_acl_tested': False, 'rpo_slo_proven': False,
    'rto_slo_proven': False, 'release_approved': False,
}, sort_keys=True))
PY
