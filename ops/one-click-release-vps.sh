#!/usr/bin/env bash
# TOR I-12: root-owned, single-command release executor.
# Installed ONCE at /usr/local/sbin/tor-one-click-release; only this fixed
# executable may be run via tor-deploy's narrowly scoped sudoers entry.
set -Eeuo pipefail
umask 077

refuse() {
  printf 'TOR_RELEASE_REFUSED: %s\n' "$*" >&2
  exit 1
}

[[ $(id -u) == 0 ]] || refuse 'root privileges required'
[[ $# == 1 && $1 =~ ^[0-9a-f]{40}$ ]] ||
  refuse 'one full 40-character commit SHA required'
sha=$1
for command in curl python3 flock sha256sum ssh-keygen stat systemctl readlink install mv; do
  command -v "$command" >/dev/null || refuse "missing $command"
done

# Refuse an untrusted copy from the deployment user's writable inbox.
[[ ! -L /usr/local/sbin/tor-one-click-release ]] ||
  refuse 'release executor must not be a symlink'
[[ $(stat -c '%U:%G:%a' /usr/local/sbin/tor-one-click-release) == root:root:755 ]] ||
  refuse 'release executor permissions changed'
[[ $(stat -c '%U:%G' /opt/tor) == root:root &&
   $(stat -c '%U:%G' /opt/tor/releases) == root:root ]] ||
  refuse 'release directories must be root owned'

exec 9>/run/lock/tor-one-click-release.lock
flock -n 9 || refuse 'another TOR release is running'

# Verify the candidate is the *present* main. The pinned go-live script also
# checks twice (before SQLx and before changing the service).
main_sha=$(
  curl -fsSL --retry 2 --max-time 15 \
    -H 'Accept: application/vnd.github+json' \
    https://api.github.com/repos/calex794/tor/git/ref/heads/main |
  python3 -c '
import json
import re
import sys
body = json.load(sys.stdin)
ref = body.get("ref")
sha = body.get("object", {}).get("sha", "")
if ref != "refs/heads/main" or not re.fullmatch(r"[a-f0-9]{40}", sha):
    sys.exit(1)
print(sha)
'
) || refuse 'cannot verify current GitHub main'
[[ "$main_sha" == "$sha" ]] || refuse 'GitHub main advanced; run the button again'

systemctl is-active --quiet tor-web.service ||
  refuse 'existing website is not active'
systemctl is-active --quiet postgresql@18-main.service ||
  refuse 'PostgreSQL is not active'
[[ $(curl -fsS --max-time 5 http://127.0.0.1:3000/readyz) == ok ]] ||
  refuse 'existing website is not healthy; manual recovery required'
[[ $(cat /var/lib/tor-release/pg-activated) == PG_ACTIVATED ]] ||
  refuse 'verified PostgreSQL production activation is missing'
config=/etc/systemd/system/tor-web.service.d/20-postgres.conf
[[ -f "$config" && ! -L "$config" ]] ||
  refuse 'production systemd configuration missing'
grep -Fxq 'Environment=TOR_DEMO_MODE=0' "$config" ||
  refuse 'RAM demo must never be activated'
grep -Fxq 'Environment=TOR_AUTH_ENABLED=1' "$config" ||
  refuse 'real authentication must remain on'
grep -Fxq 'EnvironmentFile=/etc/tor/tor-app.env' "$config" ||
  refuse 'production secrets must stay on VPS'
previous=$(readlink -f /opt/tor/current) ||
  refuse 'previous binary missing'
[[ "$previous" == /opt/tor/releases/* && -x "$previous/tor-web" ]] ||
  refuse 'previous version is outside controlled releases'

inbox="/opt/tor/incoming/$sha"
target="/opt/tor/releases/$sha"
[[ -d "$inbox" && ! -L "$inbox" ]] ||
  refuse 'verified candidate has not been delivered'
[[ $(stat -c '%U:%G' "$inbox") == tor-deploy:tor-deploy ]] ||
  refuse 'incoming candidate must be owned by tor-deploy'
for f in tor-web tor-migrate tor-web.sha256 tor-migrate.sha256 tor-release-manifest tor-release-manifest.sig; do
  [[ -f "$inbox/$f" && ! -L "$inbox/$f" ]] ||
    refuse "staged $f is missing or unsafe"
done
# An SSH deploy account may alter its own inbox, but has no private signing
# key. Verify the build manifest with the GitHub Environment's *existing*
# SSH key and a trusted allowed_signers file installed once by root.
signers=/etc/tor/tor-release-allowed-signers
[[ -f "$signers" && ! -L "$signers" &&
   $(stat -c '%U:%G:%a' "$signers") == root:root:644 ]] ||
  refuse 'trusted release signer is not configured'
ssh-keygen -Y verify -f "$signers" -I tor-release -n tor-release \
  -s "$inbox/tor-release-manifest.sig" < "$inbox/tor-release-manifest" >/dev/null ||
  refuse 'staged artifact signature is invalid'
expected_manifest=$(
  printf 'TOR_RELEASE_SHA %s\n' "$sha"
  cat "$inbox/tor-web.sha256"
  cat "$inbox/tor-migrate.sha256"
)
[[ $(cat "$inbox/tor-release-manifest") == "$expected_manifest" ]] ||
  refuse 'signed manifest does not match the selected SHA and checksums'
[[ $(cat "$inbox/tor-web.sha256") =~ ^[a-f0-9]{64}[[:space:]][[:space:]]tor-web$ ]] ||
  refuse 'unexpected tor-web checksum format'
[[ $(cat "$inbox/tor-migrate.sha256") =~ ^[a-f0-9]{64}[[:space:]][[:space:]]tor-migrate$ ]] ||
  refuse 'unexpected migrator checksum format'
(cd "$inbox" &&
  sha256sum -c --status tor-web.sha256 &&
  sha256sum -c --status tor-migrate.sha256) ||
  refuse 'staged binaries have unexpected hashes'
echo 'TOR_RELEASE_SIGNATURE_OK'

# Never execute a binary from the SSH user's writable inbox as root.
# Copy to a root-owned release directory, then independently verify the copy.
if [[ -e "$target" ]]; then
  [[ -d "$target" && ! -L "$target" &&
     $(stat -c '%U:%G' "$target") == root:root ]] ||
    refuse 'existing release directory is not trusted'
else
  temporary=$(mktemp -d "/opt/tor/releases/.candidate-$sha-XXXXXXXX")
  trap 'rm -rf -- "$temporary"' EXIT
  for f in tor-web tor-migrate; do
    install -o root -g root -m 0755 "$inbox/$f" "$temporary/$f"
  done
  for f in tor-web.sha256 tor-migrate.sha256 tor-release-manifest tor-release-manifest.sig; do
    install -o root -g root -m 0644 "$inbox/$f" "$temporary/$f"
  done
  (cd "$temporary" &&
    sha256sum -c --status tor-web.sha256 &&
    sha256sum -c --status tor-migrate.sha256) ||
    refuse 'root-owned copy failed checksum verification'
  # mktemp -d defaults to 0700: tor-web runs as tor-deploy and must
  # traverse the new root-owned release directory after switching.
  chmod 0755 "$temporary"
  mv -T "$temporary" "$target"
  trap - EXIT
fi

(cd "$target" &&
  sha256sum -c --status tor-web.sha256 &&
  sha256sum -c --status tor-migrate.sha256) ||
  refuse 'candidate release checksum mismatch'
[[ $(cat "$target/tor-release-manifest") == "$expected_manifest" ]] ||
  refuse 'root-owned release manifest differs from signed candidate'
ssh-keygen -Y verify -f "$signers" -I tor-release -n tor-release \
  -s "$target/tor-release-manifest.sig" < "$target/tor-release-manifest" >/dev/null ||
  refuse 'root-owned release signature is invalid'
[[ $(stat -c '%U:%G' "$target/tor-migrate") == root:root ]] ||
  refuse 'privileged migrator must be root owned'
[[ $(stat -c '%U:%G' "$target/tor-web") == root:root ]] ||
  refuse 'server binary must be root owned'
[[ $(stat -c '%a' "$target") == 755 ]] ||
  refuse 'server process cannot traverse release directory'

if [[ "$previous" == "$target" ]]; then
  echo "TOR_ALREADY_ACTIVE $sha"
  exit 0
fi

# One established operator procedure does the local pg_dump, verifies SQLx,
# preserves root-only credentials, checks HTTPS and rolls back the executable
# if the new server fails. Database DDL is NEVER automatically rolled back.
runner=$(mktemp /root/.tor-one-click.XXXXXXXX)
trap 'rm -f -- "$runner"' EXIT
curl --fail --silent --show-error --location --retry 2 --max-time 20 \
  "https://raw.githubusercontent.com/calex794/tor/$sha/ops/go-live-postgres-vps.sh" \
  -o "$runner" || refuse 'cannot retrieve pinned go-live'
bash -n "$runner" || refuse 'pinned go-live shell is invalid'
printf 'TOR_RELEASE_PREPARED %s\n' "$sha"
bash "$runner" "$sha"
[[ $(readlink -f /opt/tor/current) == "$target" ]] ||
  refuse 'activated release symlink does not match candidate'
[[ $(curl -fsS --max-time 5 http://127.0.0.1:3000/readyz) == ok ]] ||
  refuse 'new release failed final readiness'
printf 'TOR_ONE_CLICK_RELEASE_OK %s\n' "$sha"
