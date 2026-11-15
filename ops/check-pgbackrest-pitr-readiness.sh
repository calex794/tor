#!/usr/bin/env bash
# TOR / DR-02: read-only operator check, safe to run without altering PG config.
set -Eeuo pipefail
umask 077
refuse() { echo "TOR_PITR_NOT_READY: $*" >&2; exit 1; }
[[ $# == 0 ]] || refuse "no arguments accepted"
[[ "$(id -u)" == 0 ]] || refuse "root required for local-only inventory"

for binary in runuser psql pgbackrest pg_isready; do
  command -v "$binary" >/dev/null || refuse "missing $binary"
done
[[ -x /usr/lib/postgresql/18/bin/psql ]] ||
  refuse "PostgreSQL 18 client missing"
pg_isready -q -h /var/run/postgresql -p 5432 -d tor ||
  refuse "local TOR database inaccessible"

# Stop if another instance or wrong cluster path has been selected.
data_dir="$(runuser -u postgres -- /usr/lib/postgresql/18/bin/psql -X -At   -h /var/run/postgresql -p 5432 -d tor -c "SHOW data_directory")" ||
  refuse "cannot inspect cluster"
[[ "$data_dir" == "/var/lib/postgresql/18/main" ]] ||
  refuse "unexpected cluster: $data_dir"

settings="$(runuser -u postgres -- /usr/lib/postgresql/18/bin/psql -X -At   -F '|' -h /var/run/postgresql -p 5432 -d tor   -c "SELECT name, setting FROM pg_settings WHERE name IN ('archive_mode','archive_command','wal_level','archive_timeout') ORDER BY name")" ||
  refuse "cannot read archive settings"
[[ "$settings" == *"archive_mode|on"* ]] ||
  refuse "archive_mode is not active"
[[ "$settings" == *"archive_command|pgbackrest --stanza=tor archive-push %p"* ]] ||
  refuse "archive_command is not pinned to TOR pgBackRest stanza"
[[ "$settings" == *"wal_level|replica"* || "$settings" == *"wal_level|logical"* ]] ||
  refuse "wal_level inadequate"
# archive_timeout is measured in seconds in pg_settings.
[[ "$settings" == *"archive_timeout|900"* ]] ||
  refuse "archive_timeout not pinned to 900 seconds"

# A repository backup must exist. This only checks the catalog, not that WAL
# is continuous or that a clean restore is possible. No secrets are logged.
json="$(runuser -u postgres -- pgbackrest --stanza=tor --output=json info)" ||
  refuse "pgBackRest encrypted off-site repository inaccessible"
printf '%s' "$json" | python3 -c '
import json,sys
x=json.load(sys.stdin)
if not isinstance(x,list) or not x: sys.exit(1)
found=False
for item in x:
    if item.get("name")!="tor": continue
    if not item.get("backup"): continue
    found=True
if not found: sys.exit(1)
' || refuse "no backup in repo"
echo TOR_PITR_CONFIGURATION_PRESENT_NOT_RESTORED
