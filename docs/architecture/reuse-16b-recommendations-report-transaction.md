# REUSE-16B — F statement-only report transactions

Base: 1cc145203dedc7c42b873dc413485ab4442f323e. Scope: five existing read reports in
Recommendations (audience sources, books, groups, FLOW funnel and visits).
Only transaction setup is extracted into a private repository module.
No new cross-crate dependencies, migrations, public APIs or report SQL.

Keep REPEATABLE READ READ ONLY and SET LOCAL statement_timeout = '750ms'.
The two original spellings of the latter differ only in whitespace around
the equals sign. Inherited lock_timeout, snapshot timing, validation,
SQL/binds, error mapping and commit/drop paths remain unchanged.
Exposure, health, signal, and write paths keep their distinct setup.

Acceptance: new ignored single-connection PostgreSQL test checks settings,
backend PID, inherited lock_timeout, commit and timeout cancellation/rollback.
The shared analytics action runs it alongside existing F PG/HTTP/browser
acceptance. Source guards enforce the exact profile, caller scope and CI
wiring. Require exact-head Integration CI and Repository rules, then
post-merge CI. No deployment.
