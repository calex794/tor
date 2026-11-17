# DROP-09a — disposable Yandex Object Storage acceptance probe

Run the ignored `drop_yandex_provider` test from an approved isolated host
against a precreated private bucket named `tor-drop-probe-<unique suffix>`.
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

The dedicated DELETE probe sends a wrong ETag against its own freshly
written synthetic object. The result is observational. Both a 412 and a
subsequent successful HEAD are necessary evidence for the negative case.
When both pass, the probe tries the exact ETag and supplies the exact version
if present. It then checks that its synthetic object is absent. A 403, another error, an unconditional success or a missing object
does not demonstrate conditional protection. Physical GC remains off
until all DELETE probe checks including the version-ID check, retention, reference
fencing and independent recovery are accepted separately.

Source references: [PutObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/upload),
[GetObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/get),
[DeleteObject](https://yandex.cloud/en/docs/storage/s3/api-ref/object/delete)
and [billing](https://yandex.cloud/en/docs/storage/pricing).

This probe and the repository's MinIO checks can be run without changing
production flags, traffic or migrations. A future rollout must additionally
verify sized and class-specific samples, recent cost measurements, restore
evidence, and the rollback gates in DROP-05c before enabling a runtime switch.
