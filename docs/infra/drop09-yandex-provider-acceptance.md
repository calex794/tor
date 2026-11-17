# DROP-09a — disposable Yandex Object Storage acceptance probe

Run the ignored `drop_yandex_provider` test from an approved isolated host
against a precreated private bucket named `tor-drop-probe-<unique suffix>`
with versioning enabled.
Use a dedicated identity limited to this disposable bucket and supply its
credentials as `TOR_DROP_PROVIDER_ACCESS_KEY` and
`TOR_DROP_PROVIDER_SECRET_KEY` through the operator's secret store. Set
`TOR_DROP_PROVIDER_BUCKET` and explicitly opt in with
`TOR_DROP_PROVIDER_PROBE=YES`.

```sh
cargo test -p tor-storage --features s3 --test drop_yandex_provider \
  -- --ignored --exact drop_09_yandex_private_pack_provider_acceptance --nocapture
```

The probe uploads a synthetic two-edition Zstd archive and manifest, reads
both in full for integrity, verifies conditional overwrite rejection and
anonymous denial, then performs 24 exact verified Range reads. It prints
first-read latency, p50, p95 and payload bytes. These tests use a small
synthetic PACK. The numbers are not a production throughput estimate, a
cache hit-rate measurement or a provider invoice. Collect date, endpoint
region, network location, exact SHA, class, request counters and the actual
provider charges. Retain or expire synthetic probe objects using the
disposable bucket lifecycle.

The DELETE probe creates two synthetic versions under one unique key. A
wrong ETag on an explicit request for the older version must yield 412 and
leave the old bytes readable. It then sends the correct ETag for that same
version, checks for `404 NoSuchVersion` on a version-specific GET and reads
the newer version as current. A key-only 404 can also mean a delete marker,
so it provides no evidence of version removal. The `accepted` field is the
DELETE result independently of the read probe. Physical GC remains off
pending provider evidence, retention, reference fencing and independent
recovery.

Source references: [PutObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/upload),
[GetObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/get),
[DeleteObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/delete)
and [billing](https://yandex.cloud/en/docs/storage/pricing).

This probe and the repository's MinIO checks can be run without changing
production flags, traffic or migrations. A future rollout must additionally
verify sized and class-specific samples, recent cost measurements, restore
evidence, and the rollback gates in DROP-05c before enabling a runtime switch.
