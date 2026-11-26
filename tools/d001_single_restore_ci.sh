#!/usr/bin/env bash
# D-001 selected SINGLE recovery in the already guarded I-11 disposable lane.
# The source is a synthetic local file, vault is the separate loopback MinIO.
# No production remote, live credentials, or source S3 deletion.
d001_single_bucket=tor-dr-ci-book-single
d001_single_text='D001 historical SINGLE source V1'

d001_single_seed() {
  local fixture=$1 chapter version key
  mkdir -p "$fixture"
  psql -XAtq --no-password --dbname "$MIGRATOR_URL" -v ON_ERROR_STOP=1 -F ' ' \
    -v "original=$d001_single_text" -v "bucket=$d001_single_bucket" >"$fixture/edition" <<'SQL'
BEGIN;
INSERT INTO users(email_normalized,display_name,password_hash)
 VALUES ('d001-offsite@example.invalid','D001 offsite fixture','$argon2id$test')
 RETURNING id AS reader_id \gset
INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
 VALUES ('ci:d001:offsite-single','ru','D001 offsite SINGLE','public',:reader_id,'basic')
 RETURNING id AS book_id \gset
INSERT INTO chapters(book_id,chapter_no,title,visibility,reader_released_at)
 VALUES (:book_id,1,'D001 historical SINGLE','public',clock_timestamp())
 RETURNING id AS chapter_id \gset
INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
 VALUES (:chapter_id,1,NULL,sha256(convert_to(:'original','UTF8')))
 RETURNING id AS historical_id \gset
INSERT INTO free_chapter_storage_objects(chapter_id,version_id,bucket,object_key,sha256,size_bytes)
 VALUES (:chapter_id,:historical_id,:'bucket',
    'private/free-chapters/'||:chapter_id::text||'/'||:historical_id::text||'.md',
    sha256(convert_to(:'original','UTF8')),octet_length(:'original'));
INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
 VALUES (:chapter_id,2,'D001 current V2',sha256(convert_to('D001 current V2','UTF8')))
 RETURNING id AS current_id \gset
INSERT INTO chapter_publications(chapter_id,version_id) VALUES (:chapter_id,:current_id);
SELECT :chapter_id,:historical_id;
COMMIT;
SQL
  read -r chapter version < "$fixture/edition"
  [[ "$chapter" =~ ^[1-9][0-9]*$ && "$version" =~ ^[1-9][0-9]*$ ]]
  key="private/free-chapters/$chapter/$version.md"
  mkdir -p "$fixture/source/$(dirname "$key")"
  printf '%s' "$d001_single_text" > "$fixture/source/$key"
  python3 tools/i11_object_manifest.py snapshot --root "$fixture/source" \
    --output "$fixture/source-manifest.json"
  sha256sum "$fixture/source-manifest.json" | cut -d ' ' -f 1 > "$fixture/manifest-digest"
}

d001_single_archive() {
  local fixture=$1 vault=$2 chapter version key
  read -r chapter version < "$fixture/edition"
  key="private/free-chapters/$chapter/$version.md"
  s3 --upload-file "$fixture/source/$key" --header 'If-None-Match: *' \
    --output /dev/null "$vault/objects/$key"
  s3 --upload-file "$fixture/source-manifest.json" --header 'If-None-Match: *' \
    --output /dev/null "$vault/d001-single-manifest.json"
  rm -f -- "$fixture/source/$key" "$fixture/source-manifest.json"
  [[ ! -e "$fixture/source/$key" ]]
}

d001_single_verify_restore() {
  local fixture=$1 vault=$2 restored_url=$3 chapter version key
  read -r chapter version < "$fixture/edition"
  key="private/free-chapters/$chapter/$version.md"
  [[ ! -e "$fixture/source/$key" ]]
  mkdir -p "$fixture/restored/$(dirname "$key")"
  s3 --output "$fixture/restored/$key" "$vault/objects/$key"
  s3 --output "$fixture/retained.json" "$vault/d001-single-manifest.json"
  psql -Xq --no-password --dbname "$restored_url" \
    -v "bucket=$d001_single_bucket" -v "chapter_id=$chapter" -v "version_id=$version" \
    -f tools/d001_restore_single_receipt.sql > "$fixture/receipt.csv"
  local -a verifier=(python3 tools/drop_verify_restore_receipts.py
    --manifest "$fixture/retained.json"
    --expected-manifest-sha256 "$(cat "$fixture/manifest-digest")"
    --restored-single-root "$fixture/restored")
  "${verifier[@]}" < "$fixture/receipt.csv"
  python3 - "$fixture/restored/$key" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
body = bytearray(p.read_bytes())
body[0] ^= 1
p.write_bytes(body)
PY
  if "${verifier[@]}" < "$fixture/receipt.csv" >"$fixture/corrupt.log" 2>&1; then
    echo 'FAIL: corrupt restored SINGLE passed exact receipt verifier' >&2
    return 1
  fi
  grep -Fq 'restored SINGLE bytes mismatch' "$fixture/corrupt.log"
  s3 --output "$fixture/restored/$key" "$vault/objects/$key"
  "${verifier[@]}" < "$fixture/receipt.csv"
  echo 'D001_OFFSITE_SINGLE_EXACT_RESTORE_CI_OK'
}
