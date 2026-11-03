# A-05 / MOD-C03 — Moderator chapter publish/draft

Source of truth: `HeisLuka/newboo/backend/app/services/chapter_service.py::moderate_chapter_async` and `moderation.py`.

- NEwBOO `publish` maps to TOR `visibility=public,admin_hold=false`; `draft` maps to `visibility=draft,admin_hold=false`. Distinct `hidden/blocked` decisions from MOD-C02 remain.
- Author publishing, moderation, reader edition and paid entitlements remain independent. Only current moderator/admin roles can act, with CSRF and exact pinned state, revision and version. The current role and session are locked until commit, followed by book and chapter locks.
- First **free** publication from `chapter_working_drafts` installs the exact immutable publication pointer, deletes the working pointer and creates `chapter_first_release_events` in the same transaction. The release event attributes the verified book owner, not the acting moderator; legacy owner can be null.
- An existing public pointer is never replaced during `draft` or `publish`. This preserves historical paid purchases and earlier encrypted editions. An author working edit is not silently substituted for the historical published edition.
- A brand-new **paid** chapter cannot be made public by this moderation action without its B/G verified private-storage attachment and first release. Such a draft must first pass the existing `attach_verified_and_publish` process. This is an explicit missing parity case until a safe moderator-specific B/G orchestration exists.
- Future scheduled reader release times are respected. Book visibility/status/hold remain independent; new first free release requires an active public book.
- The same append-only `moderation_chapter_decision_events` audit now records `draft/published` transitions and the hold event audit records any hold release.
- No new public routes, no automatic refunds, no price changes, no production deployment or permanent DB migrations.
