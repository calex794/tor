# REUSE-02A — shared request authentication for paid grants

Owner: H/I with A/C/D contract preservation. S034/C-06 and S065/C-12.
Source: Doppelscan and manual review of the paid chapter and asset grant handlers.

Two POST handlers previously repeated identical session, active PostgreSQL actor,
explicit Origin, same-origin and double-submit CSRF checks. The new private
`paid_grants_http::request_auth::authenticate_grant` returns the authenticated
session and actor, or a denial status. Both handlers call it before inspecting
route IDs or granting access. Status precedence and private response headers
are preserved: missing/revoked session 401, PG failure 503, then invalid
Origin/CSRF 403. JSON extraction remains unchanged. It is not a general
authorization decision.

Resource-specific ID validation, purchase/Layer/admin entitlement,
rate/concurrency guards, S3 signing, reauthorization and first exposure/audit
remain in their previous order in each handler. No changes to migrations,
routes, rights, checkout, storage or deployment.

RIL-08 recognizes only the exact registered chapter and asset POST bindings,
verifies the call-first/deny-on-error sequence and reads the live shared helper
from the checked root. Mutation tests detect removal or bypass of the shared
gate and of individual guard conditions. The existing real PostgreSQL
paid-grant suite adds both-endpoint checks for missing/hostile Origin,
missing/mismatched CSRF, cross-site fetch and duplicate CSRF cookie, missing
session and PG failure; denials remain private and never invoke the signer.

Acceptance: final PR SHA Repository rules, Rust tests, real PG grant and role
matrix, paid-private E2E and existing browser checks; review post-merge on
actual main. No production rollout. This slice intentionally does not merge
the HTML and strict JSON policies or generalize to all routes.
