# S058 — durable scheduled release of a free chapter

Implementation scope: a chapter that is still a private, unpublished free draft
inside an already public book. Paid paid-author/S3 release is **not** included.

* Author opens `/me/books/{book}/chapters/{chapter}/release` and chooses a
  UTC time via `datetime-local`. Server accepts only an exact current
  draft revision and working version. No reader pointer is created by scheduling.
* Author may reschedule or cancel while pending. A claimed schedule rejects
  cancellation instead of promising a cancellation after publication started.
* SQLx stores one schedule per chapter, guarded by a state check, plus a
  five-minute claim lease and monotonically increasing generation. A crashed
  worker can retry after lease expiry.
* `TOR_SCHEDULED_RELEASE_WORKER_ENABLED=1` explicitly enables an opt-in
  15-second polling loop in authenticated PostgreSQL runtime. Startup fails
  when S058 migration is missing; no worker in demo or unauthenticated mode.
* The worker calls the established B-06 transactional release path. Its
  `chapter_publications` pointer and unique first-release event make replay
  idempotent; follow-up notifications continue through existing C handlers.
* If the owner, revision, book hold or working version changed before due time,
  release is refused and the schedule becomes `stale`. The author can
  correct the draft and reschedule without publishing unintended content.

Acceptance proof: disposable `tor_ci` SQLx test for ownership, cancellation,
post-deadline release, competing workers, one release event and stale revisions.
An eventual end-to-end browser/worker timing test remains separate.

No production rollout, worker activation, or migration execution outside CI.
