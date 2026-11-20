# UI-10A — genuine published-free-book visual acceptance

Status: Real-book CI plus script-free SSR preference, retaining strict private CSP.

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
- The real PostgreSQL book keeps `script-src 'none'`. The public picker creates no style cookie for the default `showcase` design, and writes a SameSite=Lax style cookie only for explicit `library`/`minimal` settings. Returning to default deletes it. The book SSR uses a server-owned `showcase` fallback and exact allowlisted cookie values, never arbitrary cookie bytes or JavaScript.
- No change to user authorization, publication, grant issuance, S3 or paid code. Real PG/Chromium asserts strict CSP, computed SSR geometry, intact book/reader links and a hidden script-only picker. The public browser test verifies the cookie writer.

## Scope boundary

This fixture has only free chapters. It proves nothing about paid chapter
presentation or checkout. UI-10B must use real PostgreSQL + MinIO with a book
containing both paid and free releases, checking the paid badges, action labels,
protected links, all three layouts and both themes without exposing ciphertext,
DEKs, bearer URLs, credentials or private text in screenshots/logs.

Accept only after exact-head PR CI and post-merge CI; no VPS deployment.
