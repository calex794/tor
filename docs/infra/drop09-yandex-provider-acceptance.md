# DROP-09a — disposable Yandex Object Storage acceptance probe

Run the ignored `drop_yandex_provider` tests from an approved isolated host
against a precreated private bucket named `tor-drop-probe-<unique suffix>`
with versioning enabled.
Set `TOR_DROP_PROVIDER_BUCKET`. Use the operator's secret store to supply
`TOR_DROP_PROVIDER_ACCESS_KEY` and `TOR_DROP_PROVIDER_SECRET_KEY` for a
dedicated identity that can write and read objects and read object versions
under `private/free-packs/provider-probe/` in this disposable bucket.

## Read and write acceptance

Run this stage with the read/write identity. This entry point only uploads
and reads synthetic objects.

```sh
TOR_DROP_PROVIDER_PROBE=READ_WRITE cargo test -p tor-storage --features s3 \
  --test drop_yandex_provider -- \
  --ignored --exact drop_09_yandex_private_pack_read_write_acceptance --nocapture
```

The probe uploads a synthetic two-edition Zstd archive and manifest, records
their version IDs, then verifies both versions with HEAD and full GET. It
checks conditional overwrite rejection without changing the current version
and anonymous denial on both current and version-specific URLs. It reads and
decodes both frames with version-specific Range GET before measuring the 24
Range reads used by the current runtime gateway. It prints
first-read latency, p50, p95 and payload bytes. These tests use a small
synthetic PACK. The numbers are not a production throughput estimate, a
cache hit-rate measurement or a provider invoice. Collect date, endpoint
region, network location, exact SHA, class, request counters and the actual
provider charges. Retain or expire synthetic probe objects using the
disposable bucket lifecycle.

The loopback MinIO regression also makes a different version current and
checks that the old version's full bytes and Range still resolve by ID.

## FND-07 free catalog trial

Use a fresh disposable PostgreSQL 18 `tor_test` on loopback with the schema
installed at the candidate commit and the restricted `tor_app` fixture role.
Provision a separate private `tor-fnd07-probe-<suffix>` bucket with versioning
enabled and an eight to forty character lowercase alphanumeric suffix. Use a
new bucket for each run. The probe checks for an empty catalog and queue, and
for a bucket with no object versions or delete markers before writing.

Supply `TOR_FND07_YANDEX_BUCKET`, `TOR_FND07_YANDEX_ACCESS_KEY`, and
`TOR_FND07_YANDEX_SECRET_KEY` from the operator's secret store. Scope the
identity to this bucket with permission to read its versioning setting and
list its objects and versions. Grant object reads and writes for
`staging/chapters/` and `private/free-chapters/`. The import worker needs
`s3:DeleteObject` on `staging/chapters/` for its own completed jobs. Retain or
expire the synthetic versions under the test bucket lifecycle. [Yandex bucket policy actions](https://yandex.cloud/en/docs/storage/s3/api-ref/policy/actions)

Run the test from the isolated operator host.

```sh
TEST_DATABASE_URL=postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_test \
TOR_FND07_YANDEX_PROBE=READ_WRITE \
  cargo test -p tor-web --features s3-free,s3-chapter-import \
  --test drop_04d_worker_pg_minio -- --ignored --exact \
  catalog_import::fnd07_ten_books_survive_worker_restart_and_reconcile_exact_s3_bytes --nocapture
```

Record the candidate SHA, run time, `FND07_YANDEX_FREE_CATALOG_OK` marker and
catalog counts. The trial imports ten FREE books, matches all 30 PostgreSQL
chapter editions to their S3 bytes, checks denial of unsigned access, and
downloads a chapter with a TOR reader grant. Paid editions, media and
independent recovery are subsequent DATA-READY gates.

## Exact-version deletion acceptance

Load separate credentials `TOR_DROP_PROVIDER_DELETE_ACCESS_KEY` and
`TOR_DROP_PROVIDER_DELETE_SECRET_KEY` for the identity scoped to
`s3:DeleteObjectVersion` on the same probe prefix. Keep the read/write
credentials available to create the fixture and check its bytes.

```sh
TOR_DROP_PROVIDER_PROBE=DELETE cargo test -p tor-storage --features s3 \
  --test drop_yandex_provider -- \
  --ignored --exact drop_09_yandex_exact_version_delete_acceptance --nocapture
```

The probe creates two synthetic versions under one unique key. The
read/write identity attempts to delete the older version with its correct
ETag and must receive `403 AccessDenied`. The older version must still be
readable. The deletion identity then tries the wrong ETag and must receive
412 while the old bytes remain readable. It sends the correct ETag for
that same version and the read/write identity verifies `404 NoSuchVersion`
for the old version and reads the newer version as current. A key-only
404 can also mean a delete marker. The `accepted` field records DELETE
acceptance independently of read/write acceptance. Physical GC requires
provider evidence, retention, reference fencing and independent recovery.

Source references: [PutObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/upload),
[GetObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/get),
[DeleteObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/delete),
[bucket policy actions](https://yandex.cloud/en/docs/storage/s3/api-ref/policy/actions)
and [billing](https://yandex.cloud/en/docs/storage/pricing).

This probe and the repository's MinIO checks can be run without changing
production flags, traffic or migrations. A future rollout must additionally
verify sized and class-specific samples, recent cost measurements, restore
evidence, and the rollback gates in DROP-05c before enabling a runtime switch.
