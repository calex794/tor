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
for executable in age rclone sha256sum mktemp stat; do
  command -v "$executable" >/dev/null || refuse 'missing executable'
done
work=$(mktemp -d /var/tmp/tor-dr03-restore.XXXXXXXX)
cleanup() {
  code=$?
  trap - EXIT
  rm -f -- "$work/object.bin" "$work/keys.age" "$work/keys.txt"
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
age -d -i "$TOR_DR_AGE_IDENTITY" -o "$work/keys.txt" "$work/keys.age" ||
  refuse 'offline identity cannot open sealed keys'
[[ -s "$work/keys.txt" ]] || refuse 'empty recovered secret bundle'
digest=$(sha256sum "$work/keys.txt" | cut -d ' ' -f 1)
[[ "$digest" == "$TOR_DR_EXPECTED_SECRET_SHA256" ]] ||
  refuse 'independent secret-bundle digest mismatch'
echo TOR_DR03_PRIVATE_OBJECT_AND_OFFLINE_KEY_RESTORED
