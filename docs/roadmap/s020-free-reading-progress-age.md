# S020/#486 — live age gate for saved free reading progress

This implements C-04 continuation and position checks following the age-protected Reader PR #487.

- `saved_state`, `continue_reading`, `save_position` and its stale/no-op fallback join the active account and enforce the current Book tier in SQL, before metadata return or cursor write.
- `basic` is allowed, `adult` needs the account's live opt-in, `pro_adult` needs an active trusted server grant; legacy NULL denies. Pending deletion removes public discovery but preserves valid direct free reading until deletion; site-blocked Books are denied.
- Held/draft/blocked Books and private/held/deleted/future-released chapters are still denied. Revocation hides positions without deleting them.
- C-09 free sync state and CAS now enforce the same live tier; its plaintext cursor-validation read locks the Book and viewer through commit. Completion event receipts and purchased-position paths are NOT covered: those may include paid chapters and require a separate policy. Pinned paid V1/V2, S3 and admin regressions remain P0 #486. No production deployment/migration.
