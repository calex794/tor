-- REC-13 B/I fail-closed gate: author-provided 'general' is a claim, not
-- verified eligibility. Existing author labels are not grandfathered in.
CREATE TABLE rec_general_audience_approvals (
    book_id bigint PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    reviewed_revision bigint NOT NULL CHECK (reviewed_revision>0),
    reviewer_user_id bigint REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at timestamptz NOT NULL DEFAULT now()
);

-- Maintain exactly the same public projection columns for all F consumers.
-- A change of author audience or related-book settings increments a.revision
-- and cannot reuse an approval from an earlier moderation snapshot.
CREATE OR REPLACE VIEW rec_public_books WITH (security_barrier=true) AS
SELECT b.id,b.title,b.target_language,b.author_name,b.genres,b.tags,b.created_at
FROM books b
JOIN book_recommendation_audiences a ON a.book_id=b.id
JOIN rec_general_audience_approvals review
  ON review.book_id=a.book_id
 AND review.reviewed_revision=a.revision
 AND review.reviewer_user_id IS NOT NULL
WHERE b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
  AND a.audience='general';
