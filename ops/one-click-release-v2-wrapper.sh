#!/bin/bash
# TOR RELEASE PROTOCOL 2
# Installed at the existing /usr/local/sbin/tor-one-click-release path.
set -euo pipefail
umask 077
[[ $EUID == 0 && $# == 1 && $1 =~ ^[0-9a-f]{40}$ ]] || {
  echo 'TOR_V2_REFUSED: one full commit SHA required' >&2
  exit 2
}
# Never fall back to the v1 release path, even during bootstrap.
[[ ! -L /var/lib/tor-release/release-format &&
   $(/usr/bin/stat -c '%U:%G:%a:%h' /var/lib/tor-release/release-format) == root:root:644:1 &&
   $(/usr/bin/cat /var/lib/tor-release/release-format) == 2 ]] || {
  echo 'TOR_V2_SETUP_REQUIRED: root bootstrap incomplete' >&2
  exit 2
}
exec /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin LANG=C.UTF-8 HOME=/root \
  /usr/bin/python3 -I -B -c 'import runpy, sys; sys.path.insert(0, "/usr/local/lib/tor-release-v2"); runpy.run_path("/usr/local/lib/tor-release-v2/release_executor_v2.py", run_name="__main__")' "$1"
