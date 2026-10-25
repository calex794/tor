#!/usr/bin/env bash
# Verify an installed TOR release can be rolled back and the active version restored.
# Run from root on VPS. Causes two short restarts of tor-web.service.
set -Eeuo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run this verification as root on the TOR VPS" >&2
  exit 1
fi
if [[ "$#" -ne 1 || ! "$1" =~ ^[0-9a-f]{40}$ ]]; then
  echo "Usage: bash ops/verify-tor-rollback.sh PREVIOUS_RELEASE_SHA" >&2
  exit 2
fi

previous="/opt/tor/releases/$1"
link="/opt/tor/current"
original="$(readlink -f "$link" || true)"
if [[ "$original" != /opt/tor/releases/* || "$original" == "$previous" ]]; then
  echo "Refusing: active release is missing, unexpected or equals previous" >&2
  exit 1
fi
for binary in "$original/tor-web" "$previous/tor-web"; do
  if [[ ! -x "$binary" ]]; then
    echo "Refusing: release executable is missing: $binary" >&2
    exit 1
  fi
done

ready() {
  for i in $(seq 1 20); do
    if systemctl is-active --quiet tor-web.service &&
       curl -fsS --max-time 2 http://127.0.0.1:3000/readyz >/dev/null; then
      return 0
    fi
    sleep 1
  done
  return 1
}

public_smoke() {
  curl -fsS --max-time 5 --resolve pandaolate.ru:443:127.0.0.1 \
    https://pandaolate.ru/catalog >/dev/null
}

switch_to() {
  local target="$1" temporary="/opt/tor/.rollback-test-$$"
  ln -s "$target" "$temporary"
  mv -Tf "$temporary" "$link"
  systemctl restart tor-web.service
}

restore_on_exit() {
  local result=$?
  trap - EXIT
  if [[ "$(readlink -f "$link" || true)" != "$original" ]]; then
    echo "Restoring original release after test..."
    if ! switch_to "$original"; then
      echo "RESTORE FAILED. See systemctl status tor-web.service" >&2
      exit 1
    fi
  fi
  if ! ready || ! public_smoke; then
    echo "RESTORE FAILED HEALTH CHECK. See journalctl -u tor-web.service" >&2
    exit 1
  fi
  echo "RESTORE_OK $(readlink -f "$link")"
  exit "$result"
}

if ! ready || ! public_smoke; then
  echo "Refusing: current release is not serving healthy HTTPS before test" >&2
  exit 1
fi

trap restore_on_exit EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

echo "Testing rollback to $previous"
switch_to "$previous"
ready
public_smoke
echo "ROLLBACK_OK $(readlink -f "$link")"

echo "Returning to active $original"
switch_to "$original"
ready
public_smoke
echo "CURRENT_OK $(readlink -f "$link")"
