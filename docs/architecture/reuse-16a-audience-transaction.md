# REUSE-16A — C audience report transactions

Base: 678604ed41f20943cc8302b0a964a34c3b02f223. Scope: seven existing Reading audience reports.
No new cross-domain dependency, query, permission, migration or public API.

The helper keeps two explicit profiles. Both run the original REPEATABLE READ
READ ONLY statement before the original timeout SQL. StatementOnly (report,
intent, canonical) leaves the session lock_timeout unchanged. StatementAndLock
(closed, book evidence, time, canonical rank) retains the single SELECT
set_config call with 750ms statement and 75ms lock timeouts. Do not replace
SELECT with SET: the first SELECT may establish the repeatable-read snapshot.
Validation, report SQL, binds, result mapping and transaction commit remain at
their existing call sites. Errors still map to AudienceError::Unavailable;
SQLx rolls back on drop.

Acceptance: new ignored single-connection PostgreSQL test checks real settings,
inherited lock_timeout, commit and statement-timeout rollback for both profiles.
It runs in the existing shared PostgreSQL reading analytics action. Existing
canonical/intent source guards follow the helper and check every profile.
Keep all existing PG/HTTP/browser checks and exact-head Integration CI,
Repository rules, then post-merge CI. No deploy.
