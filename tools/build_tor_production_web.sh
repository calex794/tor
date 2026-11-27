#!/usr/bin/env bash
# One production build profile shared by release delivery and required CI.
# Compiled capabilities are not runtime permission or feature activation.
set -euo pipefail
cargo build --locked --release -p tor-web --features 's3-covers,s3-free,s3-paid,s3-chapter-import'
expected='TOR_RELEASE_CAPABILITIES s3-covers=true s3-free=true s3-paid=true s3-chapter-import=true'
actual=$(env -i PATH="$PATH" timeout 10s target/release/tor-web --release-capabilities)
[[ "$actual" == "$expected" ]] || {
  echo 'TOR_RELEASE_REFUSED: production binary lacks required compiled capabilities' >&2
  exit 1
}
printf '%s\n' "$actual"
