# I-16 — isolated migration collision resolution on top of I-14 (Draft)

**Not approved migration numbering; test-only proposal.** This branch is
stacked on [Draft #243](https://github.com/calex794/tor/pull/243), preserving
the accepted S035/C-06/D-06 access and first-opening paths, and integrates
current `main@793b8bfc9e005fc3789d92cd492527a4eee6e5e6` via a separate
two-parent **test-branch** merge. No changes were merged into `main`.

## Exact source SQL and provisional ordering

| Original Draft | Original migration | New I-16-only version | Original Git blob SHA |
| --- | --- | --- | --- |
| #209 A-03 | `202610280013_account_login_events.sql` | `202610300018_account_login_events.sql` | `da6d0f35261462b76a408742951ee9c653696be6` |
| #189 F-09 | `202610290001_referral_signup.sql` | `202610300019_referral_signup.sql` | `147e742c3d4ae334e92b80263b1b97f8ade37362` |
| #204 D | `202610290001_direct_purchase_edition_upgrade.sql` | `202610300020_direct_purchase_edition_upgrade.sql` | `cc7167d75858c92634c0a174bff6922669f8c80f` |
| #227 C-15 | `202610290001_reader_book_follows.sql` | `202610300021_reader_book_follows.sql` | `349f0d66bb813015688276909bd732dfd007cf8f` |

Every target migration points to **the exact same Git blob as its original
Draft**, without text edits or checksum rewriting. In this Draft branch the
previous 48-migration I-14 tree becomes 52 migrations. There are no
duplicate numeric versions **within this one proposed tree**.

[Draft #251](https://github.com/calex794/tor/pull/251)'s read-only SQLx
audit code, tests and instructions are included with unchanged blobs, so
the main + paid-access integration and inventory tooling run together.

## Disposable acceptance, not an approval

The existing `I-14 upgrade exact PR base migration history` step is
intentionally preserved. Since this PR targets **the #243 Draft branch**,
CI extracts that branch's exact base migration files, applies all 48 in
a new `tor_test`, then upgrades to 52 in the **same disposable DB**,
verifying every original version, description and checksum plus a second
idempotent run. An independent follow-up test checks all four added
version/description/checksum tuples and probes their actual tables,
constraints, triggers and functions, with a read-only PostgreSQL session.

Full Rust/HTTP/S3/Chromium and PostgreSQL CI must pass on **this exact
HEAD**. A green migration test here does **not** prove that each source
PR's own Rust/frontend behavior has been integrated. It is a DDL
compatibility prototype; those features remain separately unmerged.

## Release blockers remain

The proposed `202610300018..021` values are **not global reservations**
and are **not pushed into** #189/#204/#209/#227. Those owners must approve
one final integration sequence, reconcile original PR histories, and
then retest the final tree. #243 and #178 still diverge. No production
database was inspected; #235 requires an authorized read-only audit of
every retained `_sqlx_migrations` ledger before planning deployment.

No new checkout/refund endpoints, runtime financial decision, ledger
reversal, payout, finance flags, deployment or main merge is included.
