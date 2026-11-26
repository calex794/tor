#!/usr/bin/env bash
# I-11: destructive recovery ONLY inside a disposable, mount-free container.
set -Eeuo pipefail
umask 077
export LC_ALL=C
[[ ${TOR_I11_DISPOSABLE_CONTAINER:-} == yes && -f /.dockerenv ]] || exit 2
[[ $(id -u) != 0 ]] || exit 2
[[ ${TOR_I11_FINANCE_SHA256:-} =~ ^[0-9a-f]{64}$ ]] || exit 2
[[ $TOR_I11_FINANCE_SHA256 != "$(printf '%064d' 0)" ]] || exit 2
export PATH="/usr/lib/postgresql/18/bin:$PATH"
work=$(mktemp -d /tmp/tor-i11-pitr.XXXXXXXX)
source_dir="$work/source"
restore_dir="$work/restored"
app_dir="$work/app-restored"
mkdir -p "$work"/{repo,log,lock,spool,source-socket,restore-socket,bad-socket}
time_root="$work/time-recovery"
time_dir="$time_root/pgdata"
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
  for dir in "$source_dir" "$restore_dir" "$work/bad-target" "$time_dir" "$app_dir"; do
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
for name in ('wrong-key', 'restore', 'bad-restore', 'time-restore'):
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
# Only bounded first SQL error per synthetic finance phase, without commands,
# DETAIL or data rows. The host applies its existing client redaction too.
for name in ('finance-import', 'finance-witness', 'finance-later', 'finance-restored-witness'):
    path = Path(root) / 'log' / (name + '.log')
    if not path.is_file():
        continue
    for line in path.read_text(errors='replace').splitlines():
        match = re.search(r'(?:^|: )(ERROR:|FATAL:|PANIC:)\s*(.*)', line)
        if match:
            text = match[2].replace(key, '[redacted]').replace(root, '[temp]')
            if not re.search(r'(?i)(?:cipher|password|secret|token)', text):
                text = re.sub(r'[^\x20-\x7e]', '?', text)[:300]
                print(match[1] + ' ' + name + ': ' + text)
            break
# Synthetic fixture only. Publish bounded startup diagnostics, never raw logs.
for name in ('time-start', 'time-pgctl', 'time-readiness', 'operator-replay'):
    path = (Path(root) / 'time-recovery/pglog/pg-paused-replay.log'
            if name == 'operator-replay' else Path(root) / 'log' / (name + '.log'))
    if not path.is_file():
        continue
    emitted = 0
    for line in path.read_text(errors='replace').splitlines():
        match = re.search(r'(?:FATAL|PANIC|ERROR|DETAIL|HINT|pg_ctl):\s*(.*)', line)
        if not match:
            continue
        text = match[0].replace(key, '[redacted]').replace(root, '[temp]')
        text = re.sub(r'(?i)(?:password|cipher-pass|secret|token)\s*[=:]\s*\S+', '[redacted]', text)
        text = re.sub(r'[^\x20-\x7e]', '?', text)[:360]
        print(f'I11_PITR_TIME_START {name} {text}')
        emitted += 1
        if emitted == 4:
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
# Bounded allowlisted synthetic dump, PACK bytes and session tokens are
# runtime-only. No fixture data is copied to the image or a host mount.
head -c 70254593 >"$work/runtime.tar"
python3 /opt/tor-i11-tools/i11_pitr_http_fixture.py "$work"
rm -- "$work/runtime.tar"
# LC_ALL=C controls test output, not the database encoding. Real TOR
# migrations require Unicode normalization, so never inherit SQL_ASCII.
initdb -D "$source_dir" -U postgres --auth=trust --locale=C --encoding=UTF8 >"$work/log/initdb.log"
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
CREATE ROLE tor_migrator NOLOGIN;
CREATE ROLE tor_app NOLOGIN;
CREATE DATABASE tor_ci OWNER tor_migrator;
CREATE TABLE public.i11_recovery_markers (id text PRIMARY KEY);
REVOKE ALL ON TABLE public.i11_recovery_markers FROM PUBLIC;
INSERT INTO public.i11_recovery_markers VALUES ('base_snapshot');
SQL
[[ $(sql -d tor_ci -c "SELECT current_setting('server_encoding')='UTF8' AND normalize(U&'e\\0301', NFC)=U&'\\00E9'") == t ]]
[[ $(sql -d tor_ci -c "SELECT to_regclass('public.reader_wallet_accounts') IS NULL") == t ]]
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
# RECOVERY-10b: import the actual migrated TOR fixture and real D-08
# checkout snapshot AFTER the physical base backup AND named target.
# This is a logical import into the source, not a new checkout execution.
# The later PITR MUST replay this committed schema/data import from WAL.
pg_restore --host="$work/source-socket" --port=55432 --username=postgres \
  --dbname=tor_ci --role=tor_migrator --no-owner --single-transaction --exit-on-error \
  "$work/fixture.dump" >"$work/log/finance-import.log" 2>&1
rm -- "$work/fixture.dump"
source_finance=$(sql -d tor_ci -q -c 'SET ROLE tor_app' \
  -f /opt/tor-i11-tools/i11_finance_restore_facts.sql 2>"$work/log/finance-witness.log")
[[ -n "$source_finance" ]]
[[ $(printf '%s' "$source_finance" | sha256sum | cut -d ' ' -f 1) == "$TOR_I11_FINANCE_SHA256" ]]
# Choose a real whole-second UTC point strictly between two transactions.
# These markers occur after the named-target case, so its proof is unchanged.
sql -c "INSERT INTO public.i11_recovery_markers VALUES ('time_before');" >/dev/null
time_target=$(sql -c "SELECT to_char((date_trunc('second',clock_timestamp()) + interval '2 seconds') AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') || '+00'")
[[ $time_target =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}\ [0-9]{2}:[0-9]{2}:[0-9]{2}\+00$ ]]
timeline=$(sql -c 'SELECT timeline_id FROM pg_control_checkpoint()')
[[ $timeline == 1 ]]
sql -c "SELECT pg_sleep_until('$time_target'::timestamptz + interval '1 second')" >/dev/null
sql -c "INSERT INTO public.i11_recovery_markers VALUES ('time_after');" >/dev/null
# Commit a separate LATER synthetic wallet adjustment in the real schema.
# This direct fixture-only SQL is NOT proof of a real funding provider.
sql -d tor_ci -q <<'SQL' >"$work/log/finance-later.log" 2>&1
BEGIN;
SET LOCAL ROLE tor_app;
WITH changed AS (
    UPDATE reader_wallet_accounts
    SET balance_minor=balance_minor+100
    WHERE user_id=(SELECT id FROM users WHERE email_normalized='d08_buyer_single@example.test')
      AND balance_minor=750
    RETURNING user_id,balance_minor
)
INSERT INTO reader_wallet_entries(user_id,kind,delta_minor,balance_after_minor,reference)
SELECT user_id,'verified_funding',100,balance_minor,'ci:recovery10b:after-target'
FROM changed;
COMMIT;
SQL
[[ $(sql -d tor_ci -c "SELECT count(*) FROM reader_wallet_entries e JOIN users u ON u.id=e.user_id JOIN reader_wallet_accounts a ON a.user_id=u.id WHERE u.email_normalized='d08_buyer_single@example.test' AND e.reference='ci:recovery10b:after-target' AND e.delta_minor=100 AND e.balance_after_minor=850 AND a.balance_minor=850") == 1 ]]
[[ $(sql -c 'SELECT count(*) FROM public.i11_recovery_markers') == 5 ]]
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
[[ $(restored_sql -d tor_ci -c "SELECT to_regclass('public.reader_wallet_accounts') IS NULL") == t ]]
if restored_sql -c 'SET ROLE i11_reader; SELECT * FROM public.i11_recovery_markers' >"$work/log/acl.log" 2>&1; then
  echo 'FAIL: restored role acquired forbidden table access' >&2
  exit 1
fi
grep -q 'permission denied for table i11_recovery_markers' "$work/log/acl.log"
pg_ctl -D "$restore_dir" -m fast -w -t 30 stop >/dev/null
# Keep hot standby off, then observe the eventual process exit and exact fatal.
# pg_ctl may return success in STANDBY before the unreachable target is detected.
backrest --pg1-path="$work/bad-target" --set="$label" --type=name \
  --target=tor_i11_nonexistent --target-action=promote restore >"$work/log/bad-restore.log" 2>&1
bad_start_status=0
pg_ctl -D "$work/bad-target" -l "$work/log/bad-start.log" -w -t 30 \
    -o "-c listen_addresses='' -c port=55434 -c unix_socket_directories=$work/bad-socket -c archive_mode=off -c hot_standby=off" start >"$work/log/bad-pgctl.log" 2>&1 || bad_start_status=$?
# Both early startup success and immediate failure require the same terminal proof.
wait_for_target_failure 30
# RECOVERY-03b: independently replay the same full set to a pinned UTC point.
# Request PAUSE (not promotion), then assert the actual paused state in SQL.
# The staged backup deliberately uses shutdown as the safe initial action;
# build fresh trusted startup settings instead of inheriting the source config.
# Local POSIX fixture only. Keep the operational S3/host gate unchanged.
stage_time_target() {
mkdir -p "$time_root"/{config/pgbackrest-empty,evidence,pgsocket,pglog,pgbackrest-lock,pgbackrest-spool,tablespaces}
chmod 0750 "$time_root/config" "$time_root/config/pgbackrest-empty"
time_conf="$time_root/config/pgbackrest-recovery.conf"
cat > "$time_conf" <<CONFIG
[global]
repo1-type=posix
repo1-path=$work/repo
repo1-cipher-type=aes-256-cbc
repo1-cipher-pass=$key
log-level-console=warn
log-level-file=off
log-path=$time_root/pglog
lock-path=$time_root/pgbackrest-lock
spool-path=$time_root/pgbackrest-spool
process-max=2
archive-async=n
[tor]
pg1-path=$time_dir
pg1-port=15435
pg1-socket-path=$time_root/pgsocket
pg1-user=postgres
CONFIG
chmod 0640 "$time_conf"
pgbackrest --config="$time_conf" --config-include-path="$time_root/config/pgbackrest-empty" \
  --stanza=tor --repo=1 --pg1-path="$time_dir" --set="$label" --type=time \
  --target="$time_target" --target-timeline="$timeline" \
  --target-exclusive --target-action=shutdown restore >"$work/log/time-restore.log" 2>&1
[[ -f "$time_dir/recovery.signal" && ! -e "$time_dir/standby.signal" ]]
}
stage_time_target
# Exercise actual RECOVERY-07/08/09a filesystem helpers, not a second
# handwritten startup configuration. This adapter is CI-only and does
# not invoke or bypass any operator entrypoint.
python3 /opt/tor-i11-tools/i11_pitr_prepare_config.py "$work" "$label" "$time_target" "$timeline"
[[ -s "$time_root/evidence/pg-config-receipt.json" ]]
[[ -f "$time_root/evidence/pg-original-auto.conf" ]]
[[ -f "$time_dir/postgresql.auto.conf" && ! -s "$time_dir/postgresql.auto.conf" ]]
# Start only this throwaway cluster with fresh config; no production network.
pg_ctl -D "$time_dir" -l "$work/log/time-start.log" -w -t 30 \
    -o "-c config_file=$time_root/config/pg-startup/postgresql.conf" start >"$work/log/time-pgctl.log" 2>&1
time_sql() { psql -X -v ON_ERROR_STOP=1 -At -h "$time_root/pgsocket" -p 15435 -U postgres -d postgres "$@"; }
paused=false
deadline=$((SECONDS + 30))
while ((SECONDS < deadline)); do
  if ! pg_ctl -D "$time_dir" status >/dev/null 2>&1; then
    echo 'FAIL: time-target PostgreSQL exited before replay paused' >&2
    exit 1
  fi
  if state=$(PGCONNECT_TIMEOUT=2 PGOPTIONS='-c statement_timeout=2000' \
      time_sql -c "SELECT pg_is_in_recovery() AND pg_get_wal_replay_pause_state()='paused'" 2>"$work/log/time-readiness.log"); then
    if [[ $state == t ]]; then paused=true; break; fi
  fi
  sleep 0.2
done
[[ $paused == true ]]
# Prove target configuration, isolation and transaction boundaries, not a
# readable standby alone. No pg_promote or pg_wal_replay_resume is allowed.
[[ $(time_sql -c "SELECT (current_setting('recovery_target_time')::timestamptz='$time_target'::timestamptz) AND current_setting('recovery_target_timeline')='$timeline' AND current_setting('recovery_target_inclusive')='off' AND current_setting('recovery_target_action')='pause'") == t ]]
[[ $(time_sql -c "SELECT current_setting('listen_addresses')='' AND current_setting('archive_mode')='off' AND current_setting('primary_conninfo')=''") == t ]]
[[ $(time_sql -c "SELECT current_setting('config_file')='$time_root/config/pg-startup/postgresql.conf' AND current_setting('data_directory')='$time_dir' AND current_setting('port')='15435'") == t ]]
[[ $(time_sql -c "SELECT string_agg(id,',' ORDER BY id) FROM public.i11_recovery_markers") == 'after_target,base_snapshot,before_target,time_before' ]]
[[ $(time_sql -d tor_ci -c "SELECT current_setting('server_encoding')='UTF8' AND normalize(U&'e\\0301', NFC)=U&'\\00E9'") == t ]]
# The same read-only witness runs as the real app role on the recovered
# TOR database. It requires all original purchase/finance rows and 750,
# not the later 850 balance; compare exact stable row snapshots as well.
restored_finance=$(time_sql -d tor_ci -q -c 'SET ROLE tor_app' \
  -f /opt/tor-i11-tools/i11_finance_restore_facts.sql 2>"$work/log/finance-restored-witness.log")
[[ -n "$restored_finance" && "$restored_finance" == "$source_finance" ]]
[[ $(printf '%s' "$restored_finance" | sha256sum | cut -d ' ' -f 1) == "$TOR_I11_FINANCE_SHA256" ]]
[[ $(time_sql -d tor_ci -c "SELECT count(*) FROM reader_wallet_entries WHERE reference='ci:recovery10b:after-target'") == 0 ]]
[[ $(time_sql -c "SELECT pg_last_xact_replay_timestamp() < '$time_target'::timestamptz") == t ]]
[[ $(time_sql -c "SELECT pg_is_in_recovery() AND pg_get_wal_replay_pause_state()='paused'") == t ]]
pg_ctl -D "$time_dir" -m fast -w -t 30 stop >/dev/null
[[ -f "$time_dir/recovery.signal" ]]
[[ ! -e "$time_dir/postmaster.pid" ]]
# RECOVERY-09c: independent fresh restore, not another start of the used PGDATA.
# Retain the first stopped fixture and its transaction-boundary witness.
mv -- "$time_root" "$work/time-review-complete"
stage_time_target
python3 /opt/tor-i11-tools/i11_pitr_run_paused.py "$work" "$label" "$time_target" "$timeline"
[[ -s "$time_root/evidence/pg-paused-replay-receipt.json" ]]
[[ ! -e "$time_dir/postmaster.pid" && -f "$time_dir/recovery.signal" ]]
# RECOVERY-10e-a: a third fresh restore at the same exclusive UTC cutoff.
# The earlier paused-review copies are not promoted or used for app writes.
mkdir -p "$work/app-socket"
pgbackrest --config="$conf" --stanza=tor --pg1-path="$app_dir" \
  --set="$label" --type=time --target="$time_target" --target-timeline="$timeline" \
  --target-exclusive --target-action=promote restore >"$work/log/app-restore.log" 2>&1
pg_ctl -D "$app_dir" -l "$work/log/app-start.log" -w -t 30 \
  -o "-c listen_addresses=127.0.0.1 -c port=15436 -c unix_socket_directories=$work/app-socket -c archive_mode=off -c hot_standby=off" start >/dev/null
app_sql() { psql -X -v ON_ERROR_STOP=1 -At -h "$work/app-socket" -p 15436 -U postgres -d postgres "$@"; }
app_ready=false
deadline=$((SECONDS + 30))
while ((SECONDS < deadline)); do
  pg_ctl -D "$app_dir" status >/dev/null 2>&1 || exit 1
  if state=$(PGCONNECT_TIMEOUT=2 PGOPTIONS='-c statement_timeout=2000' app_sql -c 'SELECT pg_is_in_recovery()' 2>"$work/log/app-readiness.log"); then
    if [[ $state == f ]]; then app_ready=true; break; fi
  fi
  sleep 0.2
done
[[ $app_ready == true ]]
[[ $(app_sql -c "SELECT current_setting('data_directory')='$app_dir' AND current_setting('listen_addresses')='127.0.0.1' AND current_setting('archive_mode')='off'") == t ]]
[[ $(app_sql -c "SELECT string_agg(id,',' ORDER BY id) FROM public.i11_recovery_markers") == 'after_target,base_snapshot,before_target,time_before' ]]
[[ $(app_sql -d tor_ci -c "SELECT count(*) FROM reader_wallet_entries WHERE reference='ci:recovery10b:after-target'") == 0 ]]
app_finance=$(app_sql -d tor_ci -q -c 'SET ROLE tor_app' -f /opt/tor-i11-tools/i11_finance_restore_facts.sql)
[[ -n "$app_finance" && "$app_finance" == "$source_finance" ]]
# Only the isolated promoted app copy enables LOGIN. Actual restored grants
# are retained; source and pause copies stay unchanged.
app_sql -c 'ALTER ROLE tor_app LOGIN' >/dev/null
[[ $(app_sql -c "SELECT NOT rolsuper AND NOT rolcreaterole AND NOT rolcreatedb AND NOT rolreplication FROM pg_roles WHERE rolname='tor_app'") == t ]]
bash /opt/tor-i11-tools/i11_pitr_http.sh "$work"
app_finance=$(app_sql -d tor_ci -q -c 'SET ROLE tor_app' -f /opt/tor-i11-tools/i11_finance_restore_facts.sql)
[[ "$app_finance" == "$source_finance" ]]
pg_ctl -D "$app_dir" -m fast -w -t 30 stop >/dev/null
python3 - "$started" "$pg_version" "$backrest_version" "$label" <<'PY'
import json, sys, time
print(json.dumps({
    'schema_version': 7, 'scope': 'isolated_container_fixture',
    'postgres_version': sys.argv[2], 'pgbackrest_version': sys.argv[3],
    'backup_label': sys.argv[4], 'source_removed': True,
    'base_marker_restored': True, 'post_backup_wal_marker_restored': True,
    'post_target_marker_absent': True, 'wrong_key_rejected': True,
    'unreachable_target_rejected': True, 'role_denial_preserved': True,
    'recovery_promoted': True,
    'utc_target_replay_paused': True,
    'utc_target_transaction_boundary_verified': True,
    'utc_target_no_promotion_verified': True,
    'generated_startup_config_verified_and_used': True,
    'operator_paused_replay_helpers_verified': True,
    'tor_checkout_finance_replayed_from_wal': True,
    'original_finance_snapshot_digest_matched': True,
    'tor_refund_finance_and_revocation_replayed_from_wal': True,
    'tor_historical_paid_free_layer_rows_replayed_from_wal': True,
    'post_target_wallet_change_excluded': True,
    'physical_http_primary_cutoff_verified': True,
    'physical_free_layer_pack_http_verified': True,
    'drill_elapsed_seconds': round(time.monotonic() - float(sys.argv[1]), 3),
    'production_tested': False, 'offsite_tested': False,
    'application_acl_tested': False, 'rpo_slo_proven': False,
    'rto_slo_proven': False, 'release_approved': False,
}, sort_keys=True))
PY
