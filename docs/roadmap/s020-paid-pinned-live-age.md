# S020 / #486 — pinned paid V1/V2 authoritative age check

Reference: NEwBOO `book_access_policy.py` and its library-pinned access rule.

- The existing D `tor_purchase_version_readable(actor,purchase,chapter,version)` function still requires verified immutable settlement fact, matching active library pointer, no refund, and no hard-held/deleted Book or Chapter.
- **Additional live checks:** active account; `basic` allowed; `adult` requires current opt-in, `pro_adult` requires active trusted server grant; legacy NULL fails closed. Site moderation `blocked` is a hard denial.
- **Preserve bought rights:** `books.visibility='draft'`, site `hidden` and `lifecycle_status='pending_deletion'` do not revoke the separately purchased pinned edition, provided age grant remains current. No mutation to finance ledger, library pointer or pinned version.
- C-09 purchased validation and purchased-position paths use this canonical D check. S3/Layer delivery may call independent G signing and needs separate exact-boundary proof of same live age/entitlement check before grant; this PR alone does **not** close all P0 #486.
- New disposable PostgreSQL test checks basic/nonbuyer, adult opt-in, pro grant/revoke, legacy NULL, retained pinned V1 after hiding/pending deletion, blocked site and refund.
- No production deployment or migration. Strict S020 accepted count remains 24/60 until CI and broader acceptance.
