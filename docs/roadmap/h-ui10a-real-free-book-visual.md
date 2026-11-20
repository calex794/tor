# UI-10A — genuine published-free-book visual acceptance

Status: CI-only acceptance plus narrowly scoped public-book CSP fix; based on main after UI-09 and UI-MOB-01.

## Why

UI-09 tested styles on demo /books/1 with no chapters: its primary action
was only `Вернуться к книгам`. This test runs inside the existing real PostgreSQL
and Chromium author-release journey, after two chapters were actually published
through the TOR UI and the book passed isolated site moderation.

## Evidence and invariants

- 3 actual styles (showcase/library/minimal) x 2 appearance themes
  (light/dark) x 2 real viewport sizes (390/1440), in one disposable PG CI job.
- Actual first free chapter read action stays on the first screen; two real
  public chapter links remain unchanged under appearance switches.
- Both browser sessions use the real server-generated published book, never a
  static lookalike or a mock chapter list. No horizontal overflow, no lost href,
  no menu overlap; screenshots persist as existing reader-browser artifacts.
- Restore system theme and showcase layout, reload, and confirm chapter counts.
- Public book CSP correction: PG/visitor-personalized book HTML previously inherited `script-src 'none'` and the existing style picker stayed hidden. For successful public book HTML only, allow `script-src 'self'` (and existing optional Surprise `connect-src 'self'`); preserve strict CSP on errors, private readers and every other private page. No inline or third-party script sources.
- No changes to user authorization, publication, grant issuance, S3 or paid code. Real PG/Chromium asserts the public book response CSP and visible working switcher.

## Scope boundary

This fixture has only free chapters. It proves nothing about paid chapter
presentation or checkout. UI-10B must use real PostgreSQL + MinIO with a book
containing both paid and free releases, checking the paid badges, action labels,
protected links, all three layouts and both themes without exposing ciphertext,
DEKs, bearer URLs, credentials or private text in screenshots/logs.

Accept only after exact-head PR CI and post-merge CI; no VPS deployment.
