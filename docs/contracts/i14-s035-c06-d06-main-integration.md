# I-14 — disposable main/S035 + C-06 + D-06 integration (Draft)

## Integrated graph

The Draft-only two-parent integration starts from `main@d24d29c7`, including
already merged S035 #224, C-15 continuation demand and public taxonomy migration
`202610300001_public_nonempty_taxonomy.sql`. The second parent preserves
all dependent Draft commits in #178 → #216 → #228 → #231 → #237 → #239.
This is **not** a merge to `main` and is not a deployment.

Twelve intersecting source/route/policy files were reconciled explicitly,
and all main-only changes were preserved. The route registry now has 89 route
pairs and 91 bound handlers, including S035 batch, Layer, C-06 ACK
and C-15 continuation demand.

## SQLx migration integrity

S035 already merged `202610290003_batch_chapter_purchases.sql` into main.
The *unmerged* Draft D-06 Layer refund snapshot file had the same numeric
version. I-14 **deletes only its Draft path** and recreates its unchanged SQL
blob under `202610300017_layer_refund_policy_snapshot.sql`. A later
merged main commit also took version `202610290004` for
`reader_continuation_demand`, colliding with unmerged Layer grants in #228.
The I-14 **Draft test tree only** renames the three C-06 migrations,
preserving their SQL content and dependent order:

| Original Draft migration | I-14 integration filename |
| --- | --- |
| `202610290004_layer_first_paid_grant_exposure.sql` | `202610290005_layer_first_paid_grant_exposure.sql` |
| `202610290005_direct_paid_grant_exposure.sql` | `202610290006_direct_paid_grant_exposure.sql` |
| `202610290006_paid_reader_render_confirmations.sql` | `202610290007_paid_reader_render_confirmations.sql` |

The test tree has one SQLx migration per numeric version, preserving
already-merged S035 and C-15 version ownership/checksums. All renames
require applied-migration inventory prior to any real database upgrade.

**Migration caution:** a successful clean PostgreSQL test cannot prove that
`202610290003_layer_refund_policy_snapshot.sql` has never been applied in a
persistent database. Before any release, review `_sqlx_migrations` across
every retained environment, establish installed migration identity and
checksums, and define an explicit compatibility repair if the old Draft
version was ever applied. Do not alter an applied checksum in place.

## Acceptance matrix

1. Real S035 two-chapter batch checkout creates distinct canonical purchases,
   two buyer wallet debits and pinned private editions. Checkout itself does
   not create either first-grant or first-open event.
2. Grant for purchased chapter A writes an immutable, chapter/version-specific
   first grant for A only; chapter B remains without issuance evidence.
3. An authenticated same-origin CSRF-protected simulated browser ACK for A
   requires its exact owner/purchase/chapter/pinned version, writes a first
   render event once, and cannot be replayed to change the original timestamp
   or used with B's purchase ID. B remains `Unknown`.
4. D-06 read-only audit reports `ChapterTextGrantUnconfirmed` before the
   ACK, `BrowserRenderReported` after it, `Unknown` for B; opening ACK does
   not debit/credit the buyer or add ledger entries.
5. Independent Chromium/MinIO WebCrypto tests must prove that a real reader
   sends its ACK **after** ciphertext GET, integrity checks, AES-GCM decrypt
   and DOM rendering. Tampered ciphertext does not send it.
6. Real PG18 migration discovery (including version uniqueness), S035 batch
   tests, D-05/D-06/D-08 suites, private S3 and complete required GitHub CI
   must pass on the exact final integration SHA.

An ACK is a positive client report, not proof a human read. An issued grant
without ACK and pre-cutover missing markers remain **UNKNOWN**, not grounds for
automatic refund approval or denial.

## Excluded
This PR does not pick any refund time limit or policy version, backfill old
purchases, approve refunds, reverse wallet entries, revoke entitlements as
refund settlement, create provider payouts, enable flags, merge into main or
deploy. Finance still needs purchase-time immutable policy plus a separate
SERIALIZABLE refund approval/reversal transaction and FIN-B05…B11.
Issues #235 and #236 remain authoritative release/integration gates.
The newer main integration is pinned at `d24d29c73d99edf14e05e84622acf5374367f825`; moving main needs another diff and exact-head CI. The current integration has **no acceptance** until complete CI and real S035/PG/S3 tests pass.

## Test fixture ownership

The actual S035 live-Chromium helper explicitly owns its local
`OpenOptions`/`Path` imports. The C-06 first-open batch regression obtains
its read-only PostgreSQL audit pool through `PgCatalog`, rather than
introducing a new direct `sqlx` dependency into `tor-web` tests.
Credentials remain only in isolated runner-temp files and are deleted after
the browser check. This separation keeps the S3 test root below 1000 lines.

## I-16 current-main follow-up

Newer merged main C-15 (#227) occupies route indexes 79/80 and SQLx
`202610290001_reader_book_follows.sql`. In the subsequent I-16 test-only
integration, Layer and C-06 ACK shift to indexes 81–90 and unmerged Layer
idempotency migration moves to `202610280014` with the original SQL blob.
The base upgrade must be revalidated from latest main's 42 migrations.
