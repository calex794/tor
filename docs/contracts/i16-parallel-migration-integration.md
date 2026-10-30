# I-16 — current-main integration of I-14 paid access and Draft SQLx (test only)

**Status: Draft / unapproved versions.** Latest integrated main in this tree is
`b13e1e71acb2881d062e4f8434c43744151600a0`, which has **43**
migrations. The experimental combined tree contains **53** migrations.
No source Draft was modified and no database outside isolated CI was accessed.

## Exact migration identities

The merged main now owns `202610300002_e03_reply_read_state.sql` from
#254, kept at its exact committed name and Git blob SHA. It also owns
`202610290001_reader_book_follows.sql` from
#227. Its original Git blob `349f0d66bb813015688276909bd732dfd007cf8f`
is used **once**, at the original main version. The duplicate I-16 copy
`202610300021_reader_book_follows.sql` was removed.

The old Layer Draft had used `202610290001_layer_create_idempotency.sql`.
That would collide with main's now-applied book-follow version. In this
**test tree only**, the exact unchanged Layer SQL blob
`33796721ffe417b93c14c88ab8668ed48c6553ac` is at
`202610280014_layer_create_idempotency.sql`. This version is ordered
**after** Layer table creation (`202610280013`) but **before** its term
wallet evidence (`202610290002`). The old Layer 001 file is absent.

Still-unmerged independent SQL blobs are unchanged:

| Original Draft | Provisional I-16-only filename | Git blob SHA |
| --- | --- | --- |
| #209 A-03 `202610280013_account_login_events.sql` | `202610300018_account_login_events.sql` | `da6d0f35261462b76a408742951ee9c653696be6` |
| #189 F-09 `202610290001_referral_signup.sql` | `202610300019_referral_signup.sql` | `147e742c3d4ae334e92b80263b1b97f8ade37362` |
| #204 D `202610290001_direct_purchase_edition_upgrade.sql` | `202610300020_direct_purchase_edition_upgrade.sql` | `cc7167d75858c92634c0a174bff6922669f8c80f` |

The earlier I-14 D-06 refund-policy snapshot at `202610300017`, and
C-06 Layer/direct grant and first-render migrations at 005/006/007,
are otherwise unchanged. This is not final version reservation.

## Route identity, controls and CI

Merged main's route registry entries 0–81 are kept **at their original
indexes**, including C-15 book follow/unfollow at 79/80 and E-03 reply
read-state at 81. Layer occupies 82–90 and paid browser first-opening
acknowledgment occupies 91.
The read-only route and CSRF/ownership governance is revalidated with
the merged main's book-follow and E-03 handlers and real HTTP tests.

For this PR's **main base**, existing PostgreSQL 18 upgrade CI must
install all 43 main migrations to isolated `tor_test`, upgrade that
same DB to 53 in dependency order, repeat the migrator idempotently
and retain every base version, description and SHA-384 checksum.
The I-16 read-only schema probe checks 6 version identities and
the added login, referral, edition, follows, and E-03 reply-read
table/trigger/function surfaces. All other DB/HTTP/S3/Chromium workflows must pass on **exact
current HEAD**, not an inherited green.

The I-15 read-only SQLx inventory tooling from #251 is included with
unchanged Git blobs. It never writes migration metadata or finances.

## What this DOES NOT prove

This Draft combines **DDL and current-main reader-follow runtime**, but
not all Rust/HTTP implementation from unmerged #189/#204/#209. The
original #178 and #243 histories diverge, and the specific policy
for ordinary refund approvals and transactionally atomic reversal is
still unimplemented. No real retained DB `_sqlx_migrations` inventory
was available. The operator must compare every retained DB ledger,
including old historical 001/003/004 experimental migrations,
before any release or attempted compatibility repair.

No merge into `main`, deploy, feature flag, live wallet operation,
automatic refund, external payout or settlement is authorized.
