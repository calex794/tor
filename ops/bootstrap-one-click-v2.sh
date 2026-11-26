#!/bin/bash
# One-time operator-approved installation of the v2 helper; no Rust release.
set -euo pipefail
umask 077
[[ $EUID == 0 && $# == 3 && $1 =~ ^[0-9a-f]{40}$ && $3 =~ ^[0-9a-f]{64}$ ]] || {
  echo 'Usage (root): bootstrap-one-click-v2.sh APPROVED_MAIN_SHA VHOST APPROVED_VHOST_SHA256' >&2
  exit 2
}
sha=$1
vhost=$2
pin=$3
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
tmp=$(mktemp -d /root/.tor-v2-bootstrap.XXXXXXXX)
trap 'rm -rf -- "$tmp"' EXIT
# Only explicitly reviewed helper source from the approved immutable commit,
# not code from the deployment account's writable incoming directory.
for name in release_bundle_v2.py managed_edge_release.py managed_systemd_release.py coordinated_release.py release_executor_v2.py adopt_edge_vhost.py install_release_v2.py release_ancestry.py; do
  curl --disable --fail --silent --show-error --location --proto '=https' --retry 2 --max-time 30 \
    "https://raw.githubusercontent.com/calex794/tor/$sha/tools/$name" -o "$tmp/$name"
done
curl --disable --fail --silent --show-error --location --proto '=https' --retry 2 --max-time 30 \
  "https://raw.githubusercontent.com/calex794/tor/$sha/ops/one-click-release-v2-wrapper.sh" -o "$tmp/wrapper.sh"
bash -n "$tmp/wrapper.sh"
# Isolated Python with only the root-private approved helper directory added.
exec_args=(/usr/bin/python3 -I -B -c 'import sys,runpy; root=sys.argv.pop(1); sys.path.insert(0,root); runpy.run_path(root+"/install_release_v2.py",run_name="__main__")')
"${exec_args[@]}" "$tmp" "$sha" "$vhost" "$pin"
