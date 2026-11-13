#!/usr/bin/env bash
# Independent-machine audit only: read one backed-up paid ciphertext and keys.
# Never contacts the live S3 bucket, TOR application or PostgreSQL.
set -Eeuo pipefail
umask 077
refuse() { printf 'TOR_DR03_RESTORE_REFUSED: %s\n' "$*" >&2; exit 2; }
[[ $# == 0 && $(id -u) == 0 ]] || refuse 'root and zero args required'
[[ ! -v GITHUB_ACTIONS ]] || refuse 'CI prohibited'
cfg=/etc/tor/offsite-restore-objects.env
owned() {
  [[ -f "$1" && ! -L "$1" && $(stat -c '%U:%G:%a' -- "$1") == root:root:600 ]] ||
    refuse 'root:root mode 600 regular file required'
}
owned "$cfg"
# shellcheck disable=SC1091
source "$cfg"
for setting in TOR_DR_RCLONE_CONFIG TOR_DR_OFFSITE_READ_REMOTE TOR_DR_AGE_IDENTITY TOR_DR_KEY_ARCHIVE TOR_DR_EXPECTED_OBJECT TOR_DR_EXPECTED_OBJECT_SHA256 TOR_DR_EXPECTED_SECRET_SHA256; do
  [[ -v $setting ]] || refuse 'missing independent-recovery setting'
done
[[ $TOR_DR_OFFSITE_READ_REMOTE =~ ^tor-offsite-read:[A-Za-z0-9_./-]+$ &&
   $TOR_DR_OFFSITE_READ_REMOTE != *..* ]] || refuse 'invalid read-only offsite remote'
[[ $TOR_DR_KEY_ARCHIVE =~ ^recovery-keys/[0-9]{8}T[0-9]{6}Z-[0-9a-f]{16}[.]age$ ]] ||
  refuse 'invalid key archive selector'
[[ $TOR_DR_EXPECTED_OBJECT =~ ^private/[A-Za-z0-9._/-]+$ &&
   $TOR_DR_EXPECTED_OBJECT != *..* ]] || refuse 'must verify private object'
[[ $TOR_DR_EXPECTED_OBJECT_SHA256 =~ ^[0-9a-f]{64}$ &&
   $TOR_DR_EXPECTED_SECRET_SHA256 =~ ^[0-9a-f]{64}$ ]] ||
  refuse 'independent SHA256 evidence required'
owned "$TOR_DR_RCLONE_CONFIG"
owned "$TOR_DR_AGE_IDENTITY"
for executable in age rclone sha256sum mktemp stat python3; do
  command -v "$executable" >/dev/null || refuse 'missing executable'
done
rclone --config "$TOR_DR_RCLONE_CONFIG" config dump |
  python3 -c 'import json,sys; c=json.load(sys.stdin).get("tor-offsite-read",{}); sys.exit(0 if c.get("type") == "crypt" and bool(c.get("remote")) else 2)' ||
  refuse 'offsite restore remote must be a crypt backend'
# Never place decrypted recovery keys on persistent /var/tmp.
[[ $(stat -f -c '%T' /run) == tmpfs ]] ||
  refuse '/run must be tmpfs before decrypting recovery secrets'
work=$(mktemp -d /run/tor-dr03-restore.XXXXXXXX)
cleanup() {
  code=$?
  trap - EXIT
  rm -f -- "$work/object.bin" "$work/keys.age"
  rmdir -- "$work" || code=1
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

rclone --config "$TOR_DR_RCLONE_CONFIG" copyto \
  "$TOR_DR_OFFSITE_READ_REMOTE/objects/$TOR_DR_EXPECTED_OBJECT" "$work/object.bin" --quiet ||
  refuse 'private ciphertext not found in remote backup'
[[ -s "$work/object.bin" ]] || refuse 'empty private ciphertext'
digest=$(sha256sum "$work/object.bin" | cut -d ' ' -f 1)
[[ "$digest" == "$TOR_DR_EXPECTED_OBJECT_SHA256" ]] || refuse 'private V1 hash mismatch'
rclone --config "$TOR_DR_RCLONE_CONFIG" copyto \
  "$TOR_DR_OFFSITE_READ_REMOTE/$TOR_DR_KEY_ARCHIVE" "$work/keys.age" --quiet ||
  refuse 'encrypted keys unavailable'
# Stream plaintext only through a pipe; do not persist keys on the restore host.
digest=$(age -d -i "$TOR_DR_AGE_IDENTITY" "$work/keys.age" | sha256sum | cut -d ' ' -f 1) ||
  refuse 'offline identity cannot open sealed keys'
[[ "$digest" != e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 &&
   "$digest" == "$TOR_DR_EXPECTED_SECRET_SHA256" ]] ||
  refuse 'empty secret bundle or independent digest mismatch'
echo TOR_DR03_PRIVATE_OBJECT_AND_OFFLINE_KEY_RESTORED
