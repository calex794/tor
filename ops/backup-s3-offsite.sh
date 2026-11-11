#!/usr/bin/env bash
# I-11/DR-03: manual, never CI. Append-only S3/objects and sealed recovery keys.
set -Eeuo pipefail
umask 077
refuse() { printf 'TOR_DR03_REFUSED: %s\n' "$*" >&2; exit 2; }
[[ $# == 0 && $(id -u) == 0 ]] || refuse 'root and zero arguments required'
[[ ! -v GITHUB_ACTIONS ]] || refuse 'CI prohibited'
cfg=/etc/tor/offsite-objects.env
owned() {
  [[ -f "$1" && ! -L "$1" && $(stat -c '%U:%G:%a' -- "$1") == root:root:600 ]] ||
    refuse 'root:root mode 600 regular file required'
}
owned "$cfg"
# shellcheck disable=SC1091
source "$cfg"
for setting in TOR_DR_PRIMARY_REMOTE TOR_DR_OFFSITE_REMOTE TOR_DR_RCLONE_CONFIG TOR_DR_AGE_RECIPIENT TOR_DR_SECRET_BUNDLE; do
  [[ -v $setting ]] || refuse 'missing setting'
done
[[ $TOR_DR_PRIMARY_REMOTE =~ ^tor-primary-ro:[A-Za-z0-9_./-]+$ &&
   $TOR_DR_PRIMARY_REMOTE != *..* ]] || refuse 'invalid read-only primary'
[[ $TOR_DR_OFFSITE_REMOTE =~ ^tor-offsite-crypt:[A-Za-z0-9_./-]+$ &&
   $TOR_DR_OFFSITE_REMOTE != *..* ]] || refuse 'invalid offsite crypt'
[[ $TOR_DR_AGE_RECIPIENT =~ ^age1[a-z0-9]{35,80}$ ]] || refuse 'invalid age recipient'
owned "$TOR_DR_RCLONE_CONFIG"
owned "$TOR_DR_SECRET_BUNDLE"
[[ -s "$TOR_DR_SECRET_BUNDLE" ]] || refuse 'empty recovery bundle'
for executable in age rclone flock mktemp stat sha256sum cmp python3; do
  command -v "$executable" >/dev/null || refuse 'missing required executable'
done
# An alias name alone does not guarantee encryption. Reject plaintext remotes.
rclone --config "$TOR_DR_RCLONE_CONFIG" config dump |
  python3 -c 'import json,sys; c=json.load(sys.stdin).get("tor-offsite-crypt",{}); r=c.get("remote",""); sys.exit(0 if c.get("type") == "crypt" and r.split(":")[0] not in ("", "tor-primary-ro", "tor-offsite-crypt") else 2)' ||
  refuse 'offsite remote must be a crypt backend on a separate underlying alias'
# Never open a predictable root-owned lock in a potentially shared /run/lock.
lock_dir=/run/tor-dr03
mkdir -m 700 "$lock_dir" 2>/dev/null || true
[[ -d "$lock_dir" && ! -L "$lock_dir" &&
   $(stat -c '%U:%G:%a' -- "$lock_dir") == root:root:700 ]] ||
  refuse 'unsafe offsite lock directory'
[[ ! -L "$lock_dir/offsite-objects.lock" ]] || refuse 'symlink lock refused'
exec 9>"$lock_dir/offsite-objects.lock"
flock -n 9 || refuse 'already running'
work=$(mktemp -d /run/tor-offsite-objects.XXXXXXXX)
cleanup() {
  code=$?
  trap - EXIT
  rm -f -- "$work/sealed.age" "$work/check.age"
  rmdir -- "$work" || code=1
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Entire bucket, so later key-prefix additions are included. Existing backups
# are never deleted; a changed same-key object fails without overwriting.
rclone --config "$TOR_DR_RCLONE_CONFIG" copy \
  "$TOR_DR_PRIMARY_REMOTE" "$TOR_DR_OFFSITE_REMOTE/objects" \
  --immutable --transfers 2 --checkers 4 --quiet ||
  refuse 'immutable offsite copy failed'
# Crypt backends may not expose plain hashes: verify downloaded contents.
rclone --config "$TOR_DR_RCLONE_CONFIG" check \
  "$TOR_DR_PRIMARY_REMOTE" "$TOR_DR_OFFSITE_REMOTE/objects" \
  --download --one-way --checkers 2 --quiet ||
  refuse 'offsite object verification failed'

stamp=$(date -u +%Y%m%dT%H%M%SZ)
nonce=$(python3 -c 'import secrets; print(secrets.token_hex(8))')
object="recovery-keys/$stamp-$nonce.age"
age -r "$TOR_DR_AGE_RECIPIENT" -o "$work/sealed.age" "$TOR_DR_SECRET_BUNDLE" ||
  refuse 'offline-recipient sealing failed'
[[ -s "$work/sealed.age" ]] || refuse 'empty sealed keys'
rclone --config "$TOR_DR_RCLONE_CONFIG" copyto \
  "$work/sealed.age" "$TOR_DR_OFFSITE_REMOTE/$object" --immutable --quiet ||
  refuse 'sealed-key upload failed'
rclone --config "$TOR_DR_RCLONE_CONFIG" cat \
  "$TOR_DR_OFFSITE_REMOTE/$object" > "$work/check.age" ||
  refuse 'sealed-key download failed'
cmp -s "$work/sealed.age" "$work/check.age" ||
  refuse 'sealed-key ciphertext mismatch'
printf 'TOR_DR03_S3_AND_KEYS_OFFSITE_OK %s\n' "$object"
