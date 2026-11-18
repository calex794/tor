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

The probe uploads a synthetic two-edition Zstd archive and manifest, reads
both in full for integrity, verifies their non-null version IDs and anonymous
denial, checks conditional overwrite rejection, then performs 24 exact verified
Range reads. It prints
first-read latency, p50, p95 and payload bytes. These tests use a small
synthetic PACK. The numbers are not a production throughput estimate, a
cache hit-rate measurement or a provider invoice. Collect date, endpoint
region, network location, exact SHA, class, request counters and the actual
provider charges. Retain or expire synthetic probe objects using the
disposable bucket lifecycle.

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
