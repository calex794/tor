# S020 / #486 — followed-release metadata: live recipient age gate

NEwBOO book access policy is the reference. The reader's subscription is not permission to reveal a title.

- The canonical `reader_followed_release_candidates` view now joins the actual recipient's active account in **both** Book-follow and creator-follow UNION branches.
- Current Book access: `basic` for active accounts; `adult` requires `adult_enabled`; `pro_adult` requires a live `user_level_grants` row. An unknown legacy `NULL` is denied.
- Book must remain published, active site-moderation, not held/deleted or `pending_deletion`, and chapter must remain normally released. Existing encrypted paid release verification, creator-follow time windows, and UNION deduplication are unchanged.
- `PgFollowUpdates::latest`, capped unread totals and set-read acknowledgement all consume the same view; filter before LIMIT 31/100. Revocation hides metadata immediately without deleting follows, read receipts or purchases.
- Disposable PostgreSQL CI exercises both branches with `basic`, 18+ opt-in, 21+ grant/revocation, `NULL`, `pending_deletion`, and read-state denial. Existing C-16/E-05 positive cases remain.
- No S3, paid-body delivery, or financial ledger writes. Still open: paid V1/V2/Layer/S3, sync/progress, full admin regression. No production deployment/migration. Strict 24/60 unchanged.
