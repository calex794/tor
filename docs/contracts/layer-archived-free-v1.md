# TOR D-05 / BR-060 — historical Layer free-chapter archive (implementation contract)

Status: specification and acceptance gate only. This document does **not** enable historical free-chapter delivery. Issue: #1022. Depends on merged #1016 (Layer paid historical archive) and #1020 (checkout guard); #1019 still requires automatic composition cleanup.

## Observed gap at `main@6218204c`

- `crates/web/assets/newboo-layer-reader.mjs` maps free TOC rows to `/reader/{chapter_id}`, not to a Layer-specific route. An `ordinary_deleted` book is not public, so this link may fail for an entitled historical subscriber.
- `PgFreeS3Ref::for_session` enforces `b.visibility='public'`; `for_saved_session` authorizes only an independent `reader_saved_free_chapters` pin. Neither derives authority from a Layer term.
- The merged #1016 Layer TOC labels free rows as `PublicFree` but does not verify a private free-version read grant. A TOC listing is not proof that a corresponding chapter can be opened.
- `free_reader_http::verified_grant::issue` rechecks authorization after S3 signing; maintain that two-check structure for any new private route.

## Immutable archive identity

For ordinary creator deletion, snapshot each currently reader-released and published **free** chapter's `chapter_id`, exact `chapter_version_id`, and verified storage identity at the transition to `pending_deletion` (at latest before public withdrawal). A future V2 or missing publication must not change existing archive identity. Only an immutable retained version with corresponding verified object can be served. A chapter not yet published/released at the snapshot has no Layer-free archival grant.

The snapshot is a book-level historical publication fact, **not** a per-subscriber fan-out. Do not write `reader_saved_free_chapters` or `library_purchased_chapters` on behalf of Layer users. A repo-only additive migration may be required for a durable snapshot; do not deploy it without separate authorization.

## Authorization invariant

A private free archive route may return metadata, shell, plaintext, S3 signed URL, PACK range, or redirect **only if** all checks independently succeed:

1. Authenticated actor with active account/session; CSRF + allowed Origin for a state-changing grant request.
2. A real, paid, nonrevoked Layer term owned by that actor with unexpired rights (`lifetime` or active `prepaid_30d`), using the same continuous renewal origin semantics as `PgLayerChapterEntitlements`. A renewal after a lapse is a new origin and cannot inherit old archived books.
3. The immutable pre-deletion Layer version in that paid origin chain contained this book **before** the owner deletion request. A new buyer with only the cleaned current composition cannot enumerate or read it.
4. The exact free chapter/version was published and released in that book as of the deletion snapshot, with immutable SHA/length and matching SINGLE or approved active PACK storage identity. Browser-supplied version/object keys are never authorities.
5. Live account-age, legal/T&S book block, site/admin hold, chapter hold/hard-delete, term revocation and expiry checks take precedence. Ordinary deletion alone is not a denial.
6. Repeat the actor-scoped SQL check **after** external S3 signing or PACK preparation, immediately before returning any capability. Fail closed on unavailable identity, mismatched generation, or races.

Separate path: e.g. `/read/layer/free/{chapter_id}` for the shell and `POST /api/v1/reader/layer/free/{chapter_id}/grant` for narrowly scoped delivery. Names are proposed, not implemented. An unmodified public `/reader/{id}` or public `/api/v1/chapters/{id}/free-grant` must **never** become the archive bypass. Do not grant access merely because a user can open some *paid* chapter of the book; prove the concrete book and version in that buyer's eligible origin chain.

## Reader and storage

- Private Layer TOC should distinguish archived-free from public-free, and link archived-free rows to the private shell. No direct public URL or CDN fallback.
- Prefer existing verified FREE S3 bytes or authorized server-side PACK range; never issue a whole-PACK presigned URL. Resolve physical placement independently from immutable version ID.
- Do not garbage-collect the last verified archived free object while any eligible historical term/lifetime right may still require it. Retention rules must account for revocation, legal purge and continuous renewal, not guess from current storefront composition.
- Set private no-store, noindex, no-referrer, strong CSP, XSS-safe text rendering. Never persist private signing URLs or convert Layer rights into pinned direct purchases.

## Required regression evidence (PG18 + MinIO + HTTP/Chromium)

1. Book with two free and two paid published chapters; buy Layer; owner password request -> genuine `pending_deletion` -> independent moderator `ordinary_deleted`; old buyer reads all four from private TOC and each exact frozen edition.
2. New buyer after cleanup reads only surviving books: no deleted-book metadata, TOC, shell, free body, paid grant, signed URL, or PACK sibling bytes; anonymous and unrelated users receive no archive access.
3. No pre-deletion publication, draft/release in future, changed free V2 after snapshot, wrong hash, missing SINGLE/PACK object, withdrawn active PACK: none can silently substitute a different version.
4. `prepaid_30d` expires/revokes -> denies; uninterrupted renewal preserves origin; late renewal loses deleted book; `lifetime` remains subject to live legal/age/security restrictions.
5. Concurrent revocation/hold/deletion during S3 sign -> deny on post-sign revalidation; lost HTTP response cannot create new rights or funds movement.
6. Deleting one of two Layer books must preserve unaffected paid/free rights and unchanged wallets, terms, revenue snapshots and direct pinned V1 rows.

## Work breakdown

A. Implement book-level snapshot in B/PG and immutable storage retention with disposable migration/tests.

B. Implement D authorizer keyed to historical term chain + concrete free version. Reuse exact entitlement and denial policy; test actor, origin, age and revocation in PG.

C. Implement private free delivery shell and signed-object/PACK gateway with post-sign revalidation; integrate TOC only after independent grant tests pass.

D. Run end-to-end 2-free/2-paid fixture and negative cases; require exact-head CI and exact-main post-merge. Keep #1022 open until D is accepted. Do not touch VPS, productive migrations, or real payments.
