# TOR / FND-02 — existing `tore` bucket: publisher preparation, NO DEPLOY

Status: **operator preparation only**, not proof of Yandex provider acceptance, DATA-READY,
permission to import real books, or permission to update the working VPS.

The operator has reported completion of the read-only stage (service account,
static key, `HEAD`/`LIST`, and review of current policies/CORS). TOR has
**not** independently inspected the bucket, keys, actual policy or IAM.
**The existing `tore` bucket stays in place. Do not enable S3 bucket versioning.**
Do not create a replacement production bucket or delete existing objects.

## 1. Prepare a different, restricted publisher identity

1. In Yandex Cloud IAM, create a service account `tor-s3-publisher` for
   eventual **write** operations. Keep the previously verified viewer identity
   read-only. Save the *new* static credentials only in an operator-managed
   secret vault; never in GitHub Actions, commits, PR descriptions, Notion or CI
   logs.
2. Do **not** grant `storage.admin`, `storage.editor`, policy/bucket
   administration or general `s3:*`. Yandex `storage.uploader` is **not an
   immutable-writer role**: it permits overwriting existing objects, though
   it does not grant ordinary object deletion.
3. Before granting writer access on `tore`, independently review its current
   IAM, ACL and entire Bucket Policy. Record a **private backup** of the
   existing policy, and compare the proposed policy *in full*. Applying a
   Yandex Bucket Policy **replaces** the previous policy; never paste a
   generic `Principal: "*"` allow-all example into the working bucket.
4. The intended writer must be limited to required chapter/PACK/manifest
   prefixes. Published FREE and paid edition objects must be write-once;
   `DeleteObject`, `DeleteObjectVersion`, ACL changes, versioning changes,
   policy administration and unconditional overwrite are disallowed.
   Check effective permissions, not just the role label.
5. An S023 import worker needs `DeleteObject` **only** for its *temporary*
   source under `staging/chapters/`. Never widen deletion to published
   `private/free-chapters/`, `private/free-packs/`, paid objects or archives.
   At present, TOR's cover, FREE and import clients share the default AWS SDK
   credential chain. A dedicated staging-cleanup role would require separately
   reviewed client/wiring changes. Do not enable the worker with a write
   identity whose source-cleanup permission has not been verified.

**Important:** This step creates a candidate service account only; it
does **not** authorize applying Bucket Policy, granting a broad write role,
creating real objects or switching application runtime flags.

## 2. External write-path gate (separate authorization)

The accepted [FND-02 provider runbook](fnd02-nonversioned-yandex-probe.md)
requires a **disposable, isolated Yandex acceptance environment**, using
synthetic objects and the proposed *effective* publisher policy. Its operator
probe intentionally refuses the existing `tore` name or a populated bucket.
Do not weaken that safeguard to run the test against real books.

Acceptance must record, without leaking identities/keys/object names:

- New conditional `PutObject` (`If-None-Match: *`) succeeds; a conflicting
  write fails, while an ordinary unconditional PUT is denied.
- `CopyObject` targeting an already occupied synthetic published object is
  denied. A source `X-Amz-Copy-Source-If-None-Match` condition does **not**
  protect the destination.
- Multipart initiation/completion is either denied to the publisher or
  requires the destination `If-None-Match` protection; an occupied target
  remains byte-for-byte unchanged. Also account for `UploadPartCopy`.
- Independently re-read and compare pre/post object SHA-256 and byte length.
  Anonymous full GET and Range GET are denied; approved signed requests
  succeed. Publisher cannot delete protected objects or modify bucket policy.
- Retain the previous source inventory and independent PostgreSQL/WAL +
  complete object/KEK recovery evidence, as specified by
  [I-11 #739](https://github.com/calex794/tor/issues/739).

Any permitted overwrite, missing independent proof, or broader-than-reviewed
IAM means **NO-GO**. These checks have **not** been performed by this PR.
The operational S3 bucket does not need versioning; immutable logical book
editions remain separate object keys with PostgreSQL version IDs and receipts.
Physical DELETE/GC stays OFF.

## 3. Build S3-enabled TOR without touching the server

On GitHub after the PR is merged, manually run
[**TOR S3 candidate build (NO DEPLOY)**](../../.github/workflows/tor-s3-candidate-build.yml)
on the approved commit. The workflow deliberately has no GitHub
Environment, cloud credentials, SSH, migration, systemd call or deployment.
It compiles exactly:

```bash
cargo build --release -p tor-web --features "s3-free,s3-chapter-import,s3-covers"
```

Its evidence artifact contains the exact commit SHA, feature profile and
binary checksum **but no binary or secrets**. It is not a deployable, signed
release. Existing [one-click production release](../../.github/workflows/tor-one-click-release.yml)
remains unchanged and still builds the default profile. **Do not press
"TOR — Обновить сайт" to test S3.**

A later separate I-line PR, approved only after the external gate, must
integrate the S3 features into the signed, rollback-protected release path,
with exact-main CI and checked schema compatibility.

## 4. Server runtime configuration — future activation, not now

For new installations, prefer a single DATABASE_URL line in
`/etc/tor/tor-app.env`. Existing installations following earlier guidance
may already contain supported S3 and KEK settings in the same file.
The release validates the exact local tor_app DATABASE_URL separately from
these allowlisted settings, proves the actual TCP login and preserves both
files byte-for-byte. It does not relocate, regenerate or enable keys or flags.
Unknown settings, duplicate names (including across the two files), unsafe
file permissions, malformed values and shell syntax still block the release.
After the separate external/provider, signed S3 release and DR gates, the
authorized operator may install **both** a root:root 0600
`/etc/tor/tor-runtime.env` and the root:root 0644
`/etc/systemd/system/tor-web.service.d/30-runtime.conf` from
[the fixed runtime drop-in](../../ops/tor-runtime.conf.example).
The release validates the optional pair before migrations and before restart;
it does not create, change, or remove the pair. Install no other conflicting
EnvironmentFile/drop-in. Keep the runtime file in the strict unquoted
ASCII KEY=value subset (no shell, quotes, escapes or duplicate names).
Only currently supported S3 settings and content-key settings are accepted.
The browser must never see `AWS_SECRET_ACCESS_KEY`. The AWS Rust SDK can
read standard variables:

```ini
AWS_REGION=ru-central1
AWS_ENDPOINT_URL_S3=https://storage.yandexcloud.net
AWS_ACCESS_KEY_ID=<operator-managed-publisher-key>
AWS_SECRET_ACCESS_KEY=<operator-managed-publisher-secret>
TOR_FREE_S3_BUCKET=tore
TOR_CHAPTER_IMPORT_S3_BUCKET=tore
TOR_COVER_S3_BUCKET=tore
TOR_PAID_GRANTS_ENABLED=0
TOR_PAID_AUTHOR_PUBLISH_ENABLED=0
TOR_PAID_AUTHOR_S3_ONLY_ENABLED=0
```

This is a **non-executable example**, not a request to save/enable it now.
For admin-managed S3 credentials, `TOR_S3_ADMIN_MASTER_KEY` belongs in the
same separate runtime file; use the independently retained canonical base64
encoding of a random 32-byte key. Never generate a replacement on each
release. Historical `TOR_CONTENT_KEK_V1` and optional V2 use canonical
unpadded URL-safe base64 as specified by
[the KEK rotation contract](paid-content-kek-v2-rollout.md).
Do not assume a default-feature release or old rollback binary supports
S3 flags; compatible signed builds and operational rollback acceptance
are separate requirements.
`TOR_FREE_S3_PUBLIC_ORIGIN` must be derived from the actual signed reader
GET URL/host and verified against CORS/CSP; do not guess that the SDK uses
path-style or virtual-hosted-style URLs. S3 runtime requires the matching
compiled Cargo features **and** approved credentials/permissions. A successful
build does not establish HTTP reader correctness on Yandex.

Keep keys out of the GitHub Environment `tor-staging`: it is for signed
**SSH delivery**, not the application's live AWS credential chain.
Do not start imports, paid grants, drop-pack cutover, cleanup, payments,
migrations or any restart as part of this document.

Sources: [Yandex storage roles](https://yandex.cloud/en/docs/storage/security/),
[Yandex conditional write policy](https://yandex.cloud/en/docs/storage/concepts/policy),
[Yandex bucket policy replacement](https://yandex.cloud/en/docs/storage/operations/buckets/policy),
[CopyObject destination semantics](https://yandex.cloud/en/docs/storage/s3/api-ref/object/copy).
