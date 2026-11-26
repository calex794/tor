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

d001_single_check() {
  local fixture=$1 root=$2
  python3 tools/drop_verify_restore_receipts.py \
    --manifest "$fixture/retained.json" \
    --expected-manifest-sha256 "$(cat "$fixture/manifest-digest")" \
    --restored-single-root "$root" < "$fixture/receipt.csv"
}

d001_single_download_verified() {
  local fixture=$1 vault=$2 db=$3 root=$4 chapter version key
  read -r chapter version < "$fixture/edition"
  key="private/free-chapters/$chapter/$version.md"
  [[ ! -e "$fixture/source/$key" ]]
  mkdir -p "$root/$(dirname "$key")"
  s3 --output "$root/$key" "$vault/objects/$key"
  s3 --output "$fixture/retained.json" "$vault/d001-single-manifest.json"
  psql -Xq --no-password --dbname "$db" \
    -v "bucket=$d001_single_bucket" -v "chapter_id=$chapter" -v "version_id=$version" \
    -f tools/d001_restore_single_receipt.sql > "$fixture/receipt.csv"
  d001_single_check "$fixture" "$root"
}

# Fixture-only privileged closure after a successful vault readback. The
# caller's real-provider activation and administrator restore policy remain OFF.
d001_single_seal_fixture() {
  local fixture=$1 vault=$2 chapter version key observed sealed
  if [[ "${GITHUB_ACTIONS:-}" != true ||
        "${TEST_DATABASE_URL:-}" != 'postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci' ||
        "${MIGRATOR_URL:-}" != 'postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci' ||
        "${TOR_DR_S3_ENDPOINT:-}" != http://127.0.0.1:9010 ||
        "$vault" != http://127.0.0.1:9010/tor-private-backup-ci ]]; then
    echo 'REFUSE: D001 closure requires disposable CI database and vault' >&2
    return 2
  fi
  read -r chapter version < "$fixture/edition"
  [[ "$chapter" =~ ^[1-9][0-9]*$ && "$version" =~ ^[1-9][0-9]*$ ]]
  key="private/free-chapters/$chapter/$version.md"
  d001_single_download_verified "$fixture" "$vault" "$TEST_DATABASE_URL" "$fixture/preseal"
  rm -f -- "$fixture/preseal/$key"
  [[ ! -e "$fixture/preseal/$key" ]]
  observed="$(psql -XAtq --no-password --dbname "$TEST_DATABASE_URL" -v ON_ERROR_STOP=1 \
    -v "chapter=$chapter" -v "version=$version" <<'SQL'
SELECT count(*)=1 AND min(version_id)=:version
FROM d001_record_free_single_retirement_candidates(:chapter,0,128);
SQL
)"
  [[ "$observed" == t ]]
  sealed="$(psql -XAtq --no-password --dbname "$MIGRATOR_URL" -v ON_ERROR_STOP=1 \
    -v "chapter=$chapter" -v "version=$version" <<'SQL'
SELECT d001_seal_free_single_for_retirement(:chapter,:version);
SQL
)"
  [[ "$sealed" == t ]]
  psql -XAtq --no-password --dbname "$TEST_DATABASE_URL" -v ON_ERROR_STOP=1 \
    -v "chapter=$chapter" -v "version=$version" >"$fixture/sealed-at" <<'SQL'
SELECT extract(epoch FROM sealed_at)::text
FROM d001_free_single_admission_gates
WHERE chapter_id=:chapter AND version_id=:version AND sealed_at IS NOT NULL;
SQL
  [[ -s "$fixture/sealed-at" ]]
  echo 'D001_OFFSITE_VERIFIED_BEFORE_FIXTURE_SEAL_OK'
}

d001_single_assert_restored_seal() {
  local fixture=$1 db=$2 chapter version restored admission
  if [[ "${GITHUB_ACTIONS:-}" != true ||
        "$db" != 'postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_offsite_restore_ci' ]]; then
    echo 'REFUSE: D001 restored seal check requires disposable recovery database' >&2
    return 2
  fi
  read -r chapter version < "$fixture/edition"
  restored="$(psql -XAtq --no-password --dbname "$db" -v ON_ERROR_STOP=1 \
    -v "chapter=$chapter" -v "version=$version" <<'SQL'
SELECT extract(epoch FROM g.sealed_at)::text
FROM d001_free_single_admission_gates g
JOIN d001_free_single_retirement_queue q USING(chapter_id,version_id)
JOIN chapter_publications p ON p.chapter_id=g.chapter_id
JOIN chapter_versions cur ON cur.chapter_id=p.chapter_id AND cur.id=p.version_id
WHERE g.chapter_id=:chapter AND g.version_id=:version AND g.sealed_at IS NOT NULL
  AND NOT q.candidate_now AND p.version_id<>g.version_id
  AND cur.body_markdown='D001 current V2';
SQL
)"
  [[ -n "$restored" && "$restored" == "$(cat "$fixture/sealed-at")" ]]
  admission="$(psql -XAtq --no-password --dbname "$db" -v ON_ERROR_STOP=1 \
    -v "chapter=$chapter" -v "version=$version" <<'SQL'
SELECT d001_record_free_single_delivery(chapter_id,version_id,bucket,sha256,size_bytes)
FROM free_chapter_storage_objects
WHERE chapter_id=:chapter AND version_id=:version;
SQL
)"
  [[ "$admission" == f ]]
  echo 'D001_OFFSITE_RESTORED_SEAL_AND_V2_OK'
}

d001_single_verify_restore() {
  local fixture=$1 vault=$2 restored_url=$3 chapter version key
  read -r chapter version < "$fixture/edition"
  key="private/free-chapters/$chapter/$version.md"
  [[ ! -e "$fixture/preseal/$key" ]]
  d001_single_assert_restored_seal "$fixture" "$restored_url"
  d001_single_download_verified "$fixture" "$vault" "$restored_url" "$fixture/restored"
  python3 - "$fixture/restored/$key" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
body = bytearray(p.read_bytes())
body[0] ^= 1
p.write_bytes(body)
PY
  if d001_single_check "$fixture" "$fixture/restored" >"$fixture/corrupt.log" 2>&1; then
    echo 'FAIL: corrupt restored SINGLE passed exact receipt verifier' >&2
    return 1
  fi
  grep -Fq 'restored SINGLE bytes mismatch' "$fixture/corrupt.log"
  d001_single_download_verified "$fixture" "$vault" "$restored_url" "$fixture/restored"
  echo 'D001_OFFSITE_SINGLE_EXACT_RESTORE_CI_OK'
}
