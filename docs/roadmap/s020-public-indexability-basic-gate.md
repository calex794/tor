# S020 / #486 — anonymous basic-only discovery and exact public projections

NEwBOO source policy: `book_indexability_policy.py` requires `visibility_status=active`, a live lifecycle and `access_level=basic` for anonymous indexing. Authenticated book view is a separate entitlement path; this slice does not implement session-based adult search.

## Scope
- Public search (all four keyset sorts), forward/reverse catalog browsing, showcase cards, filtered counts, public Books API list/detail, anonymous related-book targets and direct public cover variants use `visibility=public`, `visibility_status=active`, `access_level=basic`, and the existing hold/deletion checks **before pagination**. Unknown legacy NULL is not silently made basic.
- `public_catalog_statistics` and `public_catalog_facets` are exact anonymous basic-only projections. A new repo-only SQLx migration uses a books write lock and transaction to rebuild both, then atomically replaces insert/delete/update transition-table trigger functions. No old migration was rewritten.
- The same predicate handles age changes, moderation visibility changes, hold, publication and deletions without stale counts or facet leaks.
- PostgreSQL regression covers basic, adult, pro_adult and NULL books across sort directions, counts, facets, spotlights, tier upgrade and moderation state changes. Existing disposable public test fixtures are explicitly basic rather than relying on missing legacy fields.

## P0 issues still open
This is **anonymous discovery only**, not S020 completed or #486 resolved. Logged-in age-gated discovery is not built here; it must be separately tied to a live account session and verified grant, matching NEwBOO's distinction between anonymous indexability and signed-in viewing. The private reading library, referrals, recommendation ranking, paid V1/V2 and Layer assets, signed S3 delivery, and admin/Chromium access matrices remain in #486.

No production rollout, VPS change or migration execution outside disposable CI. Keep S020 partial and 24/60 unchanged.

The public cover and related-link gates deliberately expose only basic
resources. Session-aware adult book cover/related navigation requires a later,
separately authenticated read path; a successful logged-in chapter read must
not be treated as an implicit public cover entitlement.
