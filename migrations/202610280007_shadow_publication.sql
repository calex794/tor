-- Shadow / SEO-8: manual per-book opt-in; no automatic chapter-price scan.
-- Additive and default-off: deploying this migration cannot expose a book or
-- change the existing Pandaolate reader/search behavior.
ALTER TABLE books
    ADD COLUMN shadow_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN seo_index_owner TEXT NOT NULL DEFAULT 'pandaolate'
        CHECK (seo_index_owner IN ('pandaolate', 'shadow')),
    ADD CONSTRAINT books_shadow_index_requires_opt_in
        CHECK (seo_index_owner <> 'shadow' OR shadow_enabled);

-- A user choosing Shadow must not publish a draft/held/deleted book. That is
-- enforced on the reader side in the view (not by changing the author's draft).
CREATE INDEX books_shadow_public_id_idx ON books (id)
    WHERE shadow_enabled AND visibility='public'
      AND NOT admin_hold AND NOT is_deleted;

-- A security-barrier view is an intentional boundary for a future SELECT-only
-- shadow_reader role. It exposes only allowed, reader-public metadata.
CREATE VIEW shadow_public_books WITH (security_barrier = true) AS
SELECT id, source_identity, target_language, title, visibility,
       author_name, genres, seo_index_owner
FROM books
WHERE shadow_enabled
  AND visibility='public' AND NOT admin_hold AND NOT is_deleted;

-- Published chapter *metadata* only. In particular, do NOT select version
-- bodies from here: paid chapter bodies may be stored in encrypted S3 and
-- require an explicit, independently approved Shadow delivery policy.
-- Price on Pandaolate is deliberately NOT a book-level eligibility gate.
CREATE VIEW shadow_public_chapters WITH (security_barrier = true) AS
SELECT c.id, c.book_id, c.chapter_no, c.title
FROM chapters AS c
JOIN books AS b ON b.id = c.book_id
JOIN chapter_publications AS p ON p.chapter_id = c.id
WHERE b.shadow_enabled
  AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
  AND c.visibility='public' AND NOT c.admin_hold AND NOT c.is_deleted
  AND (c.reader_released_at IS NULL OR c.reader_released_at <= now());

CREATE INDEX chapters_shadow_public_order_idx
    ON chapters (book_id, chapter_no, id)
    WHERE visibility='public' AND NOT admin_hold AND NOT is_deleted;

-- Existing TOR installations grant the main app DML on NEW tables by default,
-- including potentially updatable simple views. Explicitly revoke it here.
-- The standalone migration smoke uses postgres without a tor_app role.
REVOKE ALL ON shadow_public_books, shadow_public_chapters FROM PUBLIC;
DO $shadow_view_permissions$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='tor_app') THEN
        REVOKE ALL ON shadow_public_books, shadow_public_chapters FROM tor_app;
        GRANT SELECT ON shadow_public_books, shadow_public_chapters TO tor_app;
    END IF;
END;
$shadow_view_permissions$;

COMMENT ON VIEW shadow_public_books IS
    'Manually enabled published books for Shadow; no author/payment fields';
COMMENT ON VIEW shadow_public_chapters IS
    'Published chapter metadata for Shadow, without purchased/draft plaintext';
