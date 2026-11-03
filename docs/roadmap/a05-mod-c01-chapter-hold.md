# A-05 / MOD-C01 — модерация глав: удержание

NEwBOO reference: `backend/app/web/routes/moderation.py` allows a moderator or administrator to list chapters and invoke `publish/hide/block/draft` with CSRF. The `chapter_service.moderate_chapter_async` workflow maps `hidden/blocked` to `admin_hold=True` and makes a separate edit to the chapter's status.

TOR retains the NEwBOO intent while respecting its different storage contract: `chapters.visibility` remains author-controlled `draft/public`, `chapter_publications` points to the immutable released version and `chapters.admin_hold` is a **separate stronger moderation gate**. We must not simulate `publish` by flipping a draft's hold.

## Implemented (narrow MOD-C01)

- `GET /moderation/chapters`: 40-row private metadata-only queue with keyset cursor, filters `all/hold/draft/published/no_version`, escaped book/chapter titles and one editable hold control. No chapter text, purchaser data, S3 URLs or private source bytes.
- `POST /moderation/chapters/{id}/hold`: a verified current moderator/administrator, same-origin and form CSRF; `expected_hold` ensures stale requests return 409, and removed/deleted chapters cannot be changed.
- PostgreSQL transaction locks the current account, session and role, then the selected chapter; writes only `chapters.admin_hold` and inserts an immutable audit event. Concurrent role revocation is serialized, audit errors roll back holds.
- The existing `PgReader` catalog, public reading and paid preview SQL explicitly check `NOT c.admin_hold`. No edits to buyer library, already settled purchases, price, ownership, published edition, version history, or release schedule.
- Disposable PostgreSQL + real HTTP acceptance checks active/removed moderator access, CSRF, forged origin, wrong cursor/filter, escaped titles, free and paid metadata, isolation of publication/version/price, hidden public chapter, 409 duplicate protection, and admin release.
- Separate migration and test in both `tor-integration.yml` and the manual `tor-pg-runtime.yml` wrapper. RIL registry/classifier and governed route inventory are updated.

## Still missing for full NEwBOO parity

Distinct `hidden/blocked` review states, moderator `publish/draft` transitions, report triage, moderation reasons and appeals, automatic notifications and moderator editor/reader preview. Do not mark entire A-05, S065 or chapter moderation parity complete from this narrow slice.

No deploy. No application to persistent databases. Scope limited to repository, CI and isolated `tor_ci`.

## MOD-C02 — distinct hide/block decisions (separate Draft PR)

Direct source-of-truth review: `HeisLuka/newboo/backend/app/services/chapter_service.py`
applies `publish -> published`, `hide -> hidden`, `block -> blocked`,
`draft -> draft`. `hidden` and `blocked` both enable `admin_hold`,
whereas `published` and `draft` clear it. Original `publish` requires a
current version. The original status field is mutable.

TOR now preserves the distinction between a moderator's `hidden` and
`blocked` actions as `chapters.moderation_state`. Both enforce the existing
`admin_hold`; the earlier generic `hold` remains available. A dedicated
immutable audit records every decision, including `hidden -> blocked` (when
the boolean hold does not change). The existing hold audit is retained.

This is NOT yet the full four-action parity: `publish/draft` require a separate
design around author-controlled draft/public visibility, immutable
`chapter_publications`, paid rights, and available versions. Neither is
fabricated by clearing a hold. Report triage, appeals and notifications are
still not implemented. No production deployment.
