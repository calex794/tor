# S020 / C — age gate in public and authenticated free reader SQL (slice 1)

Reference: [NEwBOO book_access_policy.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/books/policies/book_access_policy.py).

## Implemented in this slice

- Anonymous `public_chapter`, `public_book_page` (incl. TOC), and paid-preview SQL accept only `books.access_level='basic'`, with existing publication/hold/deletion checks unchanged.
- The new `PgReader::*_for_session` methods receive a **hashed session cookie** supplied by the auth-enabled web router; they never accept a claimed user/tier from query/form fields.
- In **the same SQL statement returning free chapter Markdown or book metadata**, PostgreSQL verifies `sessions.token_hash`, live expiry/revocation, `users.account_state='active'`, and: `users.adult_enabled` for `adult`, live `user_level_grants(level='pro_adult')` for `pro_adult`. Null/unknown imported tiers are denied by this path. Pro-adult alone does not substitute for an explicit adult opt-in, matching the NEwBOO policy.
- `/reader/{id}` hashes the legitimate session cookie and repeats the account check at query time. `/read/{id}` first authenticates and then calls the same session-bound SQL method. Both `/books/{id}` and `/api/v1/books/{id}/toc` pass the cookie through the database, not through client-provided privilege fields.
- The administrator's dedicated `/read/admin/{id}` path is **untouched**. When regular public reading is denied, the existing redirect can still select the separately authorized admin reader.
- A dedicated PostgreSQL regression verifies free text, TOC, metadata, paid preview, restricted/unknown book levels, spoofed cookies, adult opt-in, 21+ live grant, grant revocation, account session revocation, and hold. CI fixtures explicitly set `basic` rather than silently making old NULL data public.

## Remaining P0 surfaces — this does NOT close #486

1. Public/search/catalog and SEO discovery SQL, including materialized counts/facets and recommendations. Those must not leak restricted titles in anonymous results.
2. Purchases, paid V1/V2 and Layer grants, signed S3 assets, pinned versions, and browser shell: enforce effective age even for a valid purchase (without revoking the purchase record).
3. Library cards, bookmarks, follows, comments, notifications, cover images and alternative direct metadata/reader routes must be audited.
4. PostgreSQL + browser E2E for **actual** authenticated /reader, paid V1/V2 and admin read. This slice's PG regression covers the repository authorization decision; full surface parity remains unaccepted.
5. Only after the full read gate is proven may the S020 18+/21+ settings UI call the internal verified-upgrade method from PR #485.

This is an incremental guard, not a complete age-restriction product feature. No production DB modifications or deploys.
