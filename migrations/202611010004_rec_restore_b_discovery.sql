-- REC-13: remove the parallel administrator review gate added in 003.
-- This additive migration also upgrades databases that already ran 003; do
-- not rewrite an applied SQLx migration or require editing chapter text.
-- Only canonical B publication/access state and explicit author exclusions
-- determine public (anonymous/basic) recommendation eligibility.
CREATE OR REPLACE VIEW rec_public_books WITH (security_barrier=true) AS
SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,b.created_at
FROM books b
LEFT JOIN book_recommendation_audiences a ON a.book_id=b.id
WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
  AND b.access_level='basic'
  AND COALESCE(a.audience,'general')='general';

DROP TRIGGER IF EXISTS rec_review_revoke_book_change ON books;
DROP TRIGGER IF EXISTS rec_review_revoke_chapter_change ON chapters;
DROP TRIGGER IF EXISTS rec_review_revoke_publication_change ON chapter_publications;
DROP TRIGGER IF EXISTS rec_review_revoke_cover_change ON book_cover_slots;
DROP FUNCTION IF EXISTS rec_invalidate_review_on_content_change();
DROP FUNCTION IF EXISTS rec_invalidate_book_review(bigint);
DROP TABLE rec_general_audience_approvals;
