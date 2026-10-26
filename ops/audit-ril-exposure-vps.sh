#!/usr/bin/env bash
# RIL-09: read-only audit on the TOR VPS. Does NOT reload/restart nginx,
# change firewall rules, issue writes to PostgreSQL or reveal session data.
set -Eeuo pipefail
umask 077

if [[ $(id -u) -ne 0 ]]; then
  echo 'RIL09_AUDIT_FAIL: run locally as root to read nginx -T' >&2
  exit 2
fi
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
if [[ ! -f "$root/tools/check_ril_exposure.py" ||
      ! -f "$root/docs/contracts/ril-route-guards-v1.json" ]]; then
  echo 'RIL09_AUDIT_FAIL: run from a complete, pinned TOR checkout' >&2
  exit 2
fi
command -v nginx >/dev/null
command -v python3 >/dev/null

# Complete nginx dumps can contain sensitive paths/configuration. Never send
# them to ChatGPT, CI, Tela, GitHub Actions, or a public URL.
dump=$(mktemp /tmp/tor-ril-nginx.XXXXXXXX)
trap 'rm -f -- "$dump"' EXIT
nginx -T > "$dump" 2>&1

python3 "$root/tools/check_ril_exposure.py" \
  --nginx-dump "$dump" \
  --origin https://pandaolate.ru

# Only the bounded fingerprint and HTTP response codes are safe to record.
echo RIL09_AUDIT_OK
