# FND-02 — isolated Yandex S3 without bucket versioning

This procedure is a **manual, opt-in external provider test**, not a
deployment or a general-purpose S3 migration. Do not use an existing
bucket, a production VPS, permanent PostgreSQL, live reader data, payments,
or a production S3 credential. Tracking: [#1101](https://github.com/calex794/tor/issues/1101).

The automated MinIO acceptance is [PR #1160](https://github.com/calex794/tor/pull/1160).
MinIO SUCCESS is not external Yandex acceptance. The old
[DROP-09 runbook](drop09-yandex-provider-acceptance.md) contains
historical versioned-bucket probes and must **not** be run as the
non-versioned acceptance.

## Operator prerequisites

1. On an approved isolated host, prepare a **new and empty** private Yandex
   Object Storage bucket named `tor-fnd02-probe-<8..40 lowercase alphanumerics>`.
   Leave bucket versioning **never enabled**. The probe rejects suspended
   or enabled versioning. Do not change versioning on any existing bucket.
2. Create a restricted probe identity with bucket metadata read and list
   permission and object PUT/HEAD/GET/Range only on this bucket's
   `private/free-packs/fnd02/` prefix. The identity must **not** have
   `DeleteObject`, `DeleteObjectVersion`, ACL administration or bucket
   versioning controls. Use the operator's external secret store.
3. Configure a restrictive bucket policy for the probe prefix:
   unconditional `PutObject` without `If-None-Match` must be denied
   with 403, while a first conditional write succeeds and a second conditional
   write to the same key fails with 412. Review the Yandex
   `s3:if-none-match` policy condition and test the actual provider behavior.
   Deny anonymous GET and Range GET.
4. Ensure the bucket is empty before running the probe. It intentionally
   writes only synthetic fixtures and never deletes them. Its unique
   object keys are under `private/free-packs/fnd02/`. Use the isolated
   bucket lifecycle for eventual cleanup **after** collecting evidence;
   the test itself has no DELETE path.

## Manual opt-in

Make the following variables available through the operator's secret
environment, without printing their values:

- `TOR_FND02_YANDEX_PROBE=READ_WRITE`
- `TOR_FND02_YANDEX_BUCKET=tor-fnd02-probe-<suffix>`
- `TOR_FND02_YANDEX_ACCESS_KEY` and `TOR_FND02_YANDEX_SECRET_KEY`

Then, from the approved isolated checkout of the candidate commit:

```sh
cargo test -p tor-storage --features s3 \
  --test drop_yandex_unversioned -- \
  --ignored --exact \
  fnd02_yandex_nonversioned_pack_put_replay_full_and_range_acceptance \
  --nocapture
```

Never run this test in the default CI, or with arbitrary endpoints/buckets.
The client uses the fixed official `https://storage.yandexcloud.net`
endpoint and short bounded S3 operation timeouts. If the probe fails,
do **not** weaken checksum, privacy, conditional PUT or permission assertions
merely to claim provider acceptance.

## What success means

- The named bucket is empty and never-versioned; no versioning changes.
- A non-conditional synthetic write is denied by bucket policy.
- Synthetic two-chapter seekable PACK and sidecar are written with
  `If-None-Match: *` and fully verified byte-for-byte.
- Current HEAD/GET use exact object keys without S3 `VersionId`.
- Identical replay reconciles via readback; altered PUT returns 412 and
  altered digest cannot replace the original receipt.
- 24 private server-side Range GETs decode the right chapter; anonymous
  full GET and Range GET return 403.
- The terminal marker `FND02_YANDEX_NONVERSIONED_READ_PASS` includes
  bounded latency and traffic samples, not provider invoices or production
  SLOs. Record the exact candidate commit SHA, test bucket region,
  approved network location, date, policy review and actual provider
  billing separately, without secrets, internal IDs, signed URLs or books.

The test **does not** establish off-site independent recovery, WAL PITR,
restored historical Saved FREE/Layer pins, paid content acceptance or
production DATA-READY. Those are separate operator gates. Physical
DELETE/GC remains OFF and is **not required** merely to enable
read/write-only S3 storage with restricted IAM.
