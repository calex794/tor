#!/usr/bin/env bash
# TOR I: ONE-TIME root-only setup for "Update site" in GitHub Actions.
# Does not perform a release, run migrations, or restart the server.
set -Eeuo pipefail
umask 077

refuse() {
  printf 'TOR_ONE_CLICK_SETUP_REFUSED: %s\n' "$*" >&2
  exit 1
}
[[ $(id -u) == 0 ]] || refuse 'run as root'
[[ $# == 1 && $1 =~ ^[0-9a-f]{40}$ ]] ||
  refuse 'give the full 40-character approved main SHA'
sha=$1
id tor-deploy >/dev/null || refuse 'existing tor-deploy account not found'
for command in curl python3 visudo systemctl install chown readlink ssh-keygen flock; do
  command -v "$command" >/dev/null || refuse "missing $command"
done
[[ -d /opt/tor && ! -L /opt/tor &&
   -d /opt/tor/releases && ! -L /opt/tor/releases ]] ||
  refuse 'unexpected TOR release directories'
[[ $(readlink -f /opt/tor/current) == /opt/tor/releases/* ]] ||
  refuse 'unexpected current release'
systemctl is-active --quiet tor-web.service ||
  refuse 'do not change a broken website'
[[ $(curl -fsS --max-time 5 http://127.0.0.1:3000/readyz) == ok ]] ||
  refuse 'current website is not ready'
[[ $(cat /var/lib/tor-release/pg-activated) == PG_ACTIVATED ]] ||
  refuse 'real PostgreSQL mode has not been activated'

# Root runs this reviewed installer from an immutable approved main SHA.
# The pinned verifier below checks membership without requiring main to stop.
exec 9>/run/lock/tor-one-click-release.lock
flock -n 9 || refuse 'another TOR release is running'

helper=/usr/local/sbin/tor-one-click-release
guard=/usr/local/sbin/tor-release-ancestry
sudoers=/etc/sudoers.d/tor-one-click-release
signers=/etc/tor/tor-release-allowed-signers
[[ -d /etc/tor && ! -L /etc/tor &&
   $(stat -c '%U:%G:%a' /etc/tor) == root:root:700 ]] ||
  refuse 'root-owned /etc/tor credentials directory is missing'
[[ ! -L "$signers" ]] || refuse 'release signer file is a symlink'
authorized=/home/tor-deploy/.ssh/authorized_keys
[[ -f "$authorized" && ! -L "$authorized" ]] ||
  refuse 'authorized GitHub deploy key is missing'
# bootstrap-tor-deploy.sh added one restricted ed25519 GitHub deploy key.
# Use its public half to authenticate the binaries from Actions, with no
# additional secret, token, or approval input in GitHub.
public_key=
key_count=0
while read -r restriction kind key remainder; do
  if [[ "$restriction" == restrict && "$kind" == ssh-ed25519 &&
        "$key" =~ ^[A-Za-z0-9+/=]+$ ]]; then
    public_key="$kind $key"
    key_count=$((key_count + 1))
  fi
done < "$authorized"
[[ "$key_count" == 1 ]] ||
  refuse 'expected exactly one restricted GitHub deploy key'
signer_tmp=$(mktemp /root/.tor-release-signer.XXXXXXXX)
public_tmp=$(mktemp /root/.tor-release-public.XXXXXXXX)
printf 'tor-release %s\n' "$public_key" > "$signer_tmp"
printf '%s\n' "$public_key" > "$public_tmp"
chmod 0600 "$signer_tmp" "$public_tmp"
ssh-keygen -lf "$public_tmp" >/dev/null ||
  refuse 'invalid trusted release public key'
[[ ! -L "$helper" && ! -L "$guard" && ! -L "$sudoers" ]] ||
  refuse 'unexpected symlink at trusted setup destination'
if [[ -e "$helper" ]]; then
  grep -Fxq '# Installed by ops/bootstrap-one-click-release-vps.sh' "$helper" ||
    refuse 'unknown existing helper; do not overwrite'
fi
if [[ -e "$sudoers" ]]; then
  grep -Fxq '# TOR one-click release: restricted root helper only' "$sudoers" ||
    refuse 'unknown sudoers file; do not overwrite'
fi

download=$(mktemp /root/.tor-release-helper.XXXXXXXX)
guard_download=$(mktemp /root/.tor-release-ancestry.XXXXXXXX)
config_tmp=$(mktemp /etc/sudoers.d/.tor-one-click.XXXXXXXX)
trap 'rm -f -- "$download" "$guard_download" "$config_tmp" "$signer_tmp" "$public_tmp"' EXIT

curl -fsSL --retry 2 --max-time 20 \
  "https://raw.githubusercontent.com/calex794/tor/$sha/tools/release_ancestry.py" \
  -o "$guard_download" || refuse 'could not fetch pinned ancestry guard'
grep -Fq '# TOR: shared immutable forward-release ancestry guard.' "$guard_download" ||
  refuse 'unexpected ancestry guard content'
python3 -I "$guard_download" "$sha" --main-only ||
  refuse 'bootstrap SHA is not a verified main ancestor'

curl -fsSL --retry 2 --max-time 20 \
  "https://raw.githubusercontent.com/calex794/tor/$sha/ops/one-click-release-vps.sh" \
  -o "$download" || refuse 'could not fetch pinned helper'
bash -n "$download" || refuse 'downloaded helper is not valid Bash'
grep -Fq 'TOR I-12: root-owned, single-command release executor' "$download" ||
  refuse 'unexpected helper content'

# Review sudoers syntax *before* touching the trusted executable.
printf '%s\n' \
  '# TOR one-click release: restricted root helper only' \
  'tor-deploy ALL=(root) NOPASSWD: /usr/local/sbin/tor-one-click-release *' > "$config_tmp"
chmod 0440 "$config_tmp"
visudo -cf "$config_tmp" >/dev/null ||
  refuse 'sudoers validation failed'

# Existing running binaries must no longer be writable by the SSH deploy user.
# GitHub uploads future candidates only to tor-deploy-owned /opt/tor/incoming.
if [[ -e /opt/tor/incoming && -L /opt/tor/incoming ]]; then
  refuse 'unexpected incoming symlink'
fi
install -d -o tor-deploy -g tor-deploy -m 0700 /opt/tor/incoming
chown -R root:root /opt/tor/releases
chown root:root /opt/tor /opt/tor/releases
chmod 0755 /opt/tor /opt/tor/releases

install -o root -g root -m 0755 "$guard_download" "$guard"
install -o root -g root -m 0755 "$download" "$helper"
# Marker is checked for safe replacement on a later explicit bootstrap run.
printf '%s\n' '# Installed by ops/bootstrap-one-click-release-vps.sh' >> "$helper"
chmod 0755 "$helper"
chown root:root "$helper"
install -o root -g root -m 0644 "$signer_tmp" "$signers"
install -o root -g root -m 0440 "$config_tmp" "$sudoers"
visudo -c >/dev/null || refuse 'global sudoers validation failed'
systemctl is-active --quiet tor-web.service ||
  refuse 'website became inactive after permissions hardening'
[[ $(curl -fsS --max-time 5 http://127.0.0.1:3000/readyz) == ok ]] ||
  refuse 'website did not remain ready after setup'
echo 'TOR_ONE_CLICK_BOOTSTRAPPED'
echo 'GitHub Actions: TOR — Обновить сайт (Run workflow on main).'
echo 'Subsequent releases require no SSH, SHA input or manual SQLx commands.'
