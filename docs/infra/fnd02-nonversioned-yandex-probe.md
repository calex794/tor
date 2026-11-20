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
The probe now refuses to run when either `GITHUB_ACTIONS` or `CI` is present,
even if the explicit opt-in and provider credentials were accidentally set.
The two regular S3 lanes run only its separate pure scope-rejection unit test;
that test does not read credentials or send any network requests.
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

## FND-07: ten-book FREE catalog acceptance

This separate FND-07 probe tests the **ordinary S023 import workflow**, not
the two-chapter PACK fixture above. It must also use a new, empty,
never-versioned Yandex bucket. The previous versioned FND-07 procedure
in [DROP-09](drop09-yandex-provider-acceptance.md) is superseded.

### Isolation and access

- On an isolated operator host (not GitHub Actions, not TOR VPS), provision
  a fresh local PostgreSQL 18 `tor_test` database with the schema of the
  candidate commit and the dedicated `tor_app` fixture role. The test
  refuses any nonempty book catalog or chapter import queue and does not
  run migrations on the live server.
- Provision a **new private empty bucket** with a name matching
  `tor-fnd07-probe-<8..40 lowercase alphanumerics>`. **Never enable**
  bucket versioning. The Rust preflight reads `GetBucketVersioning` and
  requires an absent status, then verifies `ListObjectsV2` is empty.
  Enabled and Suspended statuses are both rejected. No
  `PutBucketVersioning` or provider `VersionId` is used.
- Use an isolated read/write identity with `GetBucketVersioning` and
  `ListBucket`, plus `PutObject` and private `GetObject/HeadObject`
  only on `staging/chapters/` and `private/free-chapters/` in
  this bucket.
  The **existing S023 worker** removes its successfully imported
  temporary source: it needs `DeleteObject` **only for
  `staging/chapters/`**. This exception is not permission to delete
  published editions, PACKs, manifests, purchased versions, or backups.
  Do not grant `DeleteObjectVersion`, bucket-versioning administration,
  or any broad wildcard delete right to the publishing identity.
- In a later runtime setup separate the publisher and temporary staging
  cleanup roles if IAM granularity or blast-radius analysis requires it.
  Do not change current production IAM just to run this disposable probe.
- Record actual private bucket-policy checks (anonymous GET denied;
  conditional immutable writes and non-overwrite behavior). The test
  validates object hashes and signed download, but does not itself
  constitute a complete review of bucket policy or independent recovery.

### Explicit isolated operator run

Load `TOR_FND07_YANDEX_BUCKET`, `TOR_FND07_YANDEX_ACCESS_KEY`
and `TOR_FND07_YANDEX_SECRET_KEY` from a secure, non-logging secret store.
Set `TOR_FND07_YANDEX_PROBE=READ_WRITE` for this test only. The code
requires exactly the disposable `tor_test` URL:

```sh
TEST_DATABASE_URL=postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_test \
TOR_FND07_YANDEX_PROBE=READ_WRITE \
  cargo test -p tor-web --features s3-free,s3-chapter-import \
  --test drop_04d_worker_pg_minio -- --ignored --exact \
  catalog_import::fnd07_ten_books_survive_worker_restart_and_reconcile_exact_s3_bytes --nocapture
```

The test refuses invocation through GitHub Actions, a non-loopback DB
target, a wrong bucket name, a populated catalog or a populated bucket.
Do **not** run it until the dedicated credentials, policy and test
environment have been reviewed. It writes synthetic books and objects;
cleanup is a separate approval and the test must never delete durable
editions.

Success requires the `FND07_YANDEX_FREE_CATALOG_OK` marker with
`versioning=never-enabled`, ten FREE books, thirty chapters/editions,
thirty byte-verified private objects, a clean restart/resume of the
import worker, zero missing/unexpected keys, an anonymous access denial,
and a real signed reader GET of the correct chapter. Capture sanitized
evidence, exact SHA, elapsed time, request quantities and provider costs.

**This is still not a DATA-READY sign-off.** Paid, PACK-direct bulk,
historical Saved FREE/Layer pins after independent recovery, off-site
backups, WAL PITR and production rollout gates remain separate.
