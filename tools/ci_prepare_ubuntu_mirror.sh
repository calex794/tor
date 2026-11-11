#!/usr/bin/env bash
# GitHub Ubuntu 24.04 only: retain stock mirrors unless all three DNS probes fail.
set -euo pipefail
if [[ "$(printenv GITHUB_ACTIONS)" != true || "$(printenv RUNNER_OS)" != Linux ]]; then
  printf 'FLOW CI mirror helper requires GitHub Actions on Linux\n' >&2
  exit 1
fi
flow_ci_os="$(awk -F= '$1=="ID" {gsub(/"/,"",$2); print $2}' /etc/os-release)"
flow_ci_version="$(awk -F= '$1=="VERSION_ID" {gsub(/"/,"",$2); print $2}' /etc/os-release)"
if [[ "$flow_ci_os" != ubuntu || "$flow_ci_version" != 24.04 ]]; then
  printf 'FLOW CI mirror helper requires Ubuntu 24.04\n' >&2
  exit 1
fi
for flow_ci_host in azure.archive.ubuntu.com archive.ubuntu.com security.ubuntu.com; do
  if timeout 4s getent ahosts "$flow_ci_host" >/dev/null; then
    printf 'FLOW CI: stock Ubuntu DNS is available; retaining current mirrorlist\n'
    exit 0
  fi
done
flow_ci_mirrorlist=/etc/apt/apt-mirrors.txt
flow_ci_sources=/etc/apt/sources.list.d/ubuntu.sources
flow_ci_keyring=/usr/share/keyrings/ubuntu-archive-keyring.gpg
flow_ci_mirror=https://mirrors.edge.kernel.org/ubuntu/
if [[ ! -f "$flow_ci_mirrorlist" || ! -r "$flow_ci_keyring" ]]; then
  printf 'FLOW CI: stock mirrorlist or Ubuntu archive keyring is unavailable\n' >&2
  exit 1
fi
if ! awk '$1=="URIs:" {for(i=2;i<=NF;i++) if($i=="mirror+file:/etc/apt/apt-mirrors.txt") found=1} END {exit !found}' "$flow_ci_sources"; then
  printf 'FLOW CI: Ubuntu sources do not use the expected stock mirrorlist\n' >&2
  exit 1
fi
flow_ci_dir="$(mktemp -d)"
trap 'rm -f "$flow_ci_dir/metadata" "$flow_ci_dir/mirrors"; rmdir "$flow_ci_dir"' EXIT
for flow_ci_suite in noble noble-updates noble-backports noble-security; do
  curl --fail --silent --show-error --proto '=https' --tlsv1.2 \
    --connect-timeout 3 --max-time 8 --max-filesize 1048576 --retry 0 \
    --output "$flow_ci_dir/metadata" "$flow_ci_mirror"dists/"$flow_ci_suite"/InRelease
  gpgv --keyring "$flow_ci_keyring" "$flow_ci_dir/metadata"
  if ! awk -v expected="$flow_ci_suite" '$0=="Suite: " expected {found=1} END {exit !found}' "$flow_ci_dir/metadata"; then
    printf 'FLOW CI: signed metadata does not match expected suite %s\n' "$flow_ci_suite" >&2
    exit 1
  fi
done
# TAB is required before mirror metadata; lower numeric priority is tried first.
# Keep all other mirrors, all sources/suites/components and the stock trust root.
printf '%s\tpriority:0\n' "$flow_ci_mirror" >"$flow_ci_dir/mirrors"
awk -v mirror="$flow_ci_mirror" '$1!=mirror {print}' "$flow_ci_mirrorlist" >>"$flow_ci_dir/mirrors"
sudo install -m 0644 "$flow_ci_dir/mirrors" "$flow_ci_mirrorlist"
printf 'FLOW CI: verified HTTPS Ubuntu mirror fallback enabled after stock DNS failure\n'
