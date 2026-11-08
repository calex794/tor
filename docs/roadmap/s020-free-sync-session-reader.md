# S020/#486 — authenticated free sync uses session-scoped Reader eligibility

The signed-in /api/v1/me/progress/free GET and PUT previously called \`PgReader::public_chapter\`, the **anonymous** basic-only lookup. An adult/pro_adult reader with a valid server opt-in/grant was denied 404 even while the protected /read/{id} Reader could read the chapter.

- Both existing endpoints now pass the **actual cookie session digest** to the existing \`PgReader::public_chapter_for_session\`. This calls the canonical SQL age gate in the very query that retrieves text for cursor validation. Missing/inactive session remains 401; the downstream free-state/CAS write separately checks the actor's current Book age rights.
- Anonymous/basic catalog behavior and public Reader gates are unchanged. The browser supplies no age grant.
- Existing isolated PostgreSQL browser test now checks a **different non-owner reader**, baseline 401, 18+ denied/allowed after opt-in, PUT cursor, 21+ denied/allowed after grant, revoked grant, legacy NULL denied, NEwBOO pending_deletion permitted, site blocked denied.
- This is a functional parity fix for S020, not completion of P0 #486: paid V1/V2 and Layer/S3 are tracked in independent PRs #505/#506; signed-in discovery and admin/browser full matrix remain open. No live migration or deployment.
