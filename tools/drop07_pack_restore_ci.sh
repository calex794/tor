#!/usr/bin/env bash
# Synthetic PACK fixture for tor_dr_s3_backup_smoke.sh. The caller supplies
# the disposable PostgreSQL connection, scratch directory and vault transport.

drop07_source_bucket=tor-dr-ci-book-pack
drop07_stem=private/free-packs/import/ci/drop07-restore-v1
drop07_text='DROP-07 recovered historical FREE V1'

drop07_seed_pack() {
  local fixture=$1
  mkdir -p "$fixture/source/$(dirname "$drop07_stem")"
  psql -XAtq --no-password --dbname "$MIGRATOR_URL" -v ON_ERROR_STOP=1 -F ' ' \
    -v "original=$drop07_text" >"$fixture/edition" <<'SQL'
BEGIN;
INSERT INTO users(email_normalized,display_name,password_hash)
 VALUES ('drop07-backup@example.invalid','DROP-07 recovery CI','$argon2id$test')
 RETURNING id AS reader_id \gset
INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
 VALUES ('test:drop07:backup','ru','DROP-07 recovery CI','public',:reader_id,'basic')
 RETURNING id AS book_id \gset
INSERT INTO chapters(book_id,chapter_no,title,visibility,reader_released_at)
 VALUES (:book_id,1,'Recovered FREE chapter','public',clock_timestamp())
 RETURNING id AS chapter_id \gset
INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
 VALUES (:chapter_id,1,NULL,sha256(convert_to(:'original','UTF8')))
 RETURNING id AS historical_id \gset
SELECT :chapter_id,:historical_id;
COMMIT;
SQL
  local chapter version pack_sha manifest_sha pack_bytes manifest_bytes frame_bytes raw_bytes
  read -r chapter version < "$fixture/edition"
  python3 - "$fixture" "$drop07_source_bucket" "$drop07_stem" "$chapter" "$version" "$drop07_text" \
    >"$fixture/descriptor" <<'PY'
import hashlib, json, struct, sys
from pathlib import Path

fixture, bucket, key_stem, chapter, version, text = sys.argv[1:]
stem = Path(fixture) / "source" / key_stem
raw = text.encode()
# Fixed zstd CLI output for the synthetic V1 body. The real Rust reader
# checks these bytes, including the frame checksum and decoded SHA-256.
frame = bytes.fromhex("28b52ffd045821010044524f502d3037207265636f766572656420"
                      "686973746f726963616c2046524545205631d2798f05")
payload = struct.pack("<IIIBI", len(frame), len(raw), 1, 0, 0x8F92EAB1)
seek = struct.pack("<II", 0x184D2A5E, len(payload)) + payload
pack = frame + seek
pack_digest = hashlib.sha256(pack).hexdigest()
sidecar = json.dumps({
    "format": "tor-store-public-seekable-zstd", "manifest_version": 1,
    "zstd_seekable_version": "0.1.0", "pack_sha256": pack_digest,
    "pack_encoded_bytes": len(pack), "seek_table_offset": len(frame),
    "seek_table_bytes": len(seek), "frames": 1,
    "entries": [{"edition_id": f"{chapter}:{version}", "frame": 0, "offset": 0,
                 "encoded_bytes": len(frame), "raw_bytes": len(raw),
                 "sha256": hashlib.sha256(raw).hexdigest()}],
}).encode()
stem.with_suffix(".zst").write_bytes(pack)
stem.with_suffix(".manifest.json").write_bytes(sidecar)
receipt = {
    "target": {"bucket": bucket, "pack_key": key_stem + ".zst",
               "manifest_key": key_stem + ".manifest.json",
               "pack_sha256": list(hashlib.sha256(pack).digest()),
               "manifest_sha256": list(hashlib.sha256(sidecar).digest()),
               "pack_bytes": len(pack), "manifest_bytes": len(sidecar),
               "seek_table_start": len(frame)},
    "frames": [{"chapter_id": int(chapter), "version_id": int(version),
                "content_sha256": list(hashlib.sha256(raw).digest()),
                "size_bytes": len(raw), "frame_offset": 0, "frame_bytes": len(frame)}],
}
(Path(fixture) / "receipt.json").write_text(json.dumps(receipt), encoding="utf-8")
print(pack_digest, hashlib.sha256(sidecar).hexdigest(), len(pack), len(sidecar), len(frame), len(raw))
PY
  read -r pack_sha manifest_sha pack_bytes manifest_bytes frame_bytes raw_bytes < "$fixture/descriptor"
  psql -XAtq --no-password --dbname "$MIGRATOR_URL" -v ON_ERROR_STOP=1 \
    -v "bucket=$drop07_source_bucket" -v "stem=$drop07_stem" -v "chapter=$chapter" \
    -v "version=$version" -v "pack_sha=$pack_sha" -v "manifest_sha=$manifest_sha" \
    -v "pack_bytes=$pack_bytes" -v "manifest_bytes=$manifest_bytes" -v "frame_bytes=$frame_bytes" \
    -v "raw_bytes=$raw_bytes" -v "receipt=$(cat "$fixture/receipt.json")" \
    >"$fixture/pack-id" <<'SQL'
BEGIN;
INSERT INTO free_storage_packs(bucket,object_key,pack_sha256,manifest_key,manifest_sha256,
   pack_bytes,manifest_bytes,seek_table_start)
 VALUES (:'bucket',:'stem'||'.zst',decode(:'pack_sha','hex'),:'stem'||'.manifest.json',
   decode(:'manifest_sha','hex'),:'pack_bytes'::bigint,:'manifest_bytes'::bigint,:'frame_bytes'::bigint)
 RETURNING id AS pack_id \gset
INSERT INTO free_edition_storage_generations(chapter_id,version_id,generation,placement_kind,
   content_sha256,size_bytes,pack_id,frame_offset,frame_bytes)
 SELECT chapter_id,id,1,'pack',content_sha256,:raw_bytes,:pack_id,0,:frame_bytes
   FROM chapter_versions WHERE chapter_id=:chapter AND id=:version;
WITH imported AS (
  INSERT INTO chapter_import_jobs(book_id,actor_user_id,command_key,source_key,source_filename,
    source_sha256,source_bytes,approved_fingerprint,expected_chapter_count,status,saved_count,finished_at)
   SELECT b.id,b.owner_user_id,repeat('D',43),'staging/chapters/ci/drop07/source-v1.txt',
     'chapters.txt',v.content_sha256,:raw_bytes,sha256('ci:drop07:pack'::bytea),1,
     'succeeded',1,clock_timestamp()
    FROM chapter_versions v JOIN chapters c ON c.id=v.chapter_id JOIN books b ON b.id=c.book_id
    WHERE v.id=:version AND c.id=:chapter
   RETURNING id,actor_user_id,book_id,approved_fingerprint
)
INSERT INTO free_pack_direct_import_stages(job_id,actor_user_id,book_id,approved_fingerprint,
  chapter_count,bucket,object_key,pack_sha256,manifest_sha256,receipt)
 SELECT id,actor_user_id,book_id,approved_fingerprint,1,:'bucket',:'stem'||'.zst',
   decode(:'pack_sha','hex'),decode(:'manifest_sha','hex'),:'receipt'::jsonb
 FROM imported;
SELECT promote_free_edition_storage(:chapter,:version,0,1) AS promoted \gset
INSERT INTO chapter_publications(chapter_id,version_id) VALUES (:chapter,:version);
SELECT :pack_id;
COMMIT;
SQL
  python3 tools/i11_object_manifest.py snapshot --root "$fixture/source" \
    --output "$fixture/source-manifest.json"
  sha256sum "$fixture/source-manifest.json" | cut -d ' ' -f 1 > "$fixture/manifest-digest"
  DROP07_RIGHTS_FILE="$fixture/rights.json" \
    cargo test -p tor-web --features s3-free --test fnd05_restored_free_rights_pg_s3 seed_free_archive \
      -- --exact --ignored --test-threads=1
}

drop07_archive_pack() {
  local fixture=$1 vault=$2 key
  for key in "$drop07_stem.zst" "$drop07_stem.manifest.json"; do
    s3 --upload-file "$fixture/source/$key" --header 'If-None-Match: *' \
      --output /dev/null "$vault/objects/$key"
  done
  s3 --upload-file "$fixture/source-manifest.json" --header 'If-None-Match: *' \
    --output /dev/null "$vault/objects-manifest.json"
  rm -f -- "$fixture/source/$drop07_stem.zst" "$fixture/source/$drop07_stem.manifest.json" \
    "$fixture/source-manifest.json"
}

drop07_verify_restored_pack() {
  local fixture=$1 vault=$2 restored_url=$3 key pack_id chapter version
  local restored_bucket="${vault%/*}/$drop07_source_bucket"
  pack_id="$(cat "$fixture/pack-id")"
  read -r chapter version < "$fixture/edition"
  mkdir -p "$fixture/restored/$(dirname "$drop07_stem")"
  for key in "$drop07_stem.zst" "$drop07_stem.manifest.json"; do
    s3 --output "$fixture/restored/$key" "$vault/objects/$key"
  done
  s3 --output "$fixture/retained.json" "$vault/objects-manifest.json"
  psql -Xq --no-password --dbname "$restored_url" -v "bucket=$drop07_source_bucket" \
    -v "pack_id=$pack_id" -f tools/drop_restore_pack_receipts.sql >"$fixture/receipts.csv"
  local -a verifier=(python3 tools/drop_verify_restore_receipts.py --manifest "$fixture/retained.json"
    --expected-manifest-sha256 "$(cat "$fixture/manifest-digest")"
    --restored-pack-root "$fixture/restored")
  "${verifier[@]}" <"$fixture/receipts.csv"
  psql -Xq --no-password --dbname "$restored_url" -v "pack_id=$pack_id" \
    -f tools/drop07_export_pack_retention.sql >"$fixture/retention.csv"
  psql -XAtq --no-password --dbname "$restored_url" -v ON_ERROR_STOP=1 \
    -v "chapter=$chapter" -v "version=$version" >"$fixture/no-fallback" <<'SQL'
SELECT body_markdown IS NULL AND NOT EXISTS (
    SELECT 1 FROM free_chapter_storage_objects WHERE chapter_id=:chapter AND version_id=:version
) FROM chapter_versions WHERE chapter_id=:chapter AND id=:version;
SQL
  grep -qx t "$fixture/no-fallback"
  python3 - "$fixture/retention.csv" "$chapter" "$version" <<'PY'
import csv, sys

with open(sys.argv[1], newline="") as f:
    rows = list(csv.DictReader(f))
fields = ("chapter_id", "version_id", "active_here", "saved_free", "layer_archive",
          "current_publication", "retention")
assert len(rows) == 1 and tuple(rows[0][field] for field in fields) == (
    sys.argv[2], sys.argv[3], "t", "t", "t", "t", "RETAIN"
), "restored PACK lost its exact protected historical edition"
PY
  local -a pack_reader=(cargo run --quiet -p tor-storage --example drop_pack_retention_verify --
    "$pack_id" "$fixture/retention.csv" "$fixture/restored/$drop07_stem.zst"
    "$fixture/restored/$drop07_stem.manifest.json" "$drop07_source_bucket" "$drop07_stem.zst")
  "${pack_reader[@]}" | grep -Fx "DROP07_PACK_RETENTION_MATCH pack_id=$pack_id frames=1 retain=1 unknown=0"

  # Serve the exact registered key from a fresh private bucket using only
  # bytes recovered from the backup vault and validated above.
  s3 --request PUT --output /dev/null "$restored_bucket"
  for key in "$drop07_stem.zst" "$drop07_stem.manifest.json"; do
    s3 --upload-file "$fixture/restored/$key" --header 'If-None-Match: *' \
      --output /dev/null "$restored_bucket/$key"
  done
  TOR_LAYER_FREE_ARCHIVE_ENABLED=1 DROP07_RIGHTS_FILE="$fixture/rights.json" \
    DROP07_RESTORED_DATABASE_URL="$restored_url" \
    cargo test -p tor-web --features s3-free --test fnd05_restored_free_rights_pg_s3 verify_restored_free_archive \
      -- --exact --ignored --test-threads=1 --show-output | tee "$fixture/rights-check.log"
  grep -Fxq 'FND05_RESTORED_FREE_ARCHIVE_HTTP_OK' "$fixture/rights-check.log"

  python3 - "$fixture/restored/$drop07_stem.zst" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
data = bytearray(path.read_bytes())
data[0] ^= 1
path.write_bytes(data)
PY
  if "${verifier[@]}" <"$fixture/receipts.csv" >"$fixture/corrupt.log" 2>&1; then
    echo 'FAIL: corrupt restored PACK passed the receipt and bytes check' >&2
    return 1
  fi
  grep -Fq 'restored PACK or manifest bytes mismatch' "$fixture/corrupt.log"
  if "${pack_reader[@]}" >"$fixture/corrupt-frame.log" 2>&1; then
    echo 'FAIL: corrupt restored PACK passed the real frame verifier' >&2
    return 1
  fi
  grep -Fq 'PACK or manifest failed full verification' "$fixture/corrupt-frame.log"
  echo 'DEPLOY07C_CORRUPT_RESTORED_PACK_DENIED'
  echo 'DROP07_RESTORED_PACK_HISTORICAL_V1_OK'
}
