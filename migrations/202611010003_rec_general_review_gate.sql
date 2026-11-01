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


-- A recommendation-settings revision is not a content revision.  Invalidate
-- reviewed eligibility on B/G content mutations as well as audience edits.
-- Serialize every invalidation with the book-row lock held by the A-authorized
-- approval transaction; a concurrent edit cannot leave a stale approval.
CREATE FUNCTION rec_invalidate_book_review(_book_id bigint)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF _book_id IS NULL THEN
        RETURN;
    END IF;
    PERFORM 1 FROM books WHERE id=_book_id FOR UPDATE;
    DELETE FROM rec_general_audience_approvals WHERE book_id=_book_id;
END;
$$;

CREATE FUNCTION rec_invalidate_review_on_content_change()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    changed_book bigint;
BEGIN
    IF TG_TABLE_NAME='books' THEN
        -- Book title, visibility, taxonomy, owner and administrative changes.
        PERFORM rec_invalidate_book_review(NEW.id);
    ELSIF TG_TABLE_NAME='chapters' THEN
        IF TG_OP='DELETE' THEN
            PERFORM rec_invalidate_book_review(OLD.book_id);
        ELSE
            IF TG_OP='UPDATE' AND OLD.book_id IS DISTINCT FROM NEW.book_id THEN
                PERFORM rec_invalidate_book_review(OLD.book_id);
            END IF;
            PERFORM rec_invalidate_book_review(NEW.book_id);
        END IF;
    ELSIF TG_TABLE_NAME='chapter_publications' THEN
        -- New releases and a changed immutable-version pointer both count.
        IF TG_OP IN ('UPDATE','DELETE') THEN
            SELECT c.book_id INTO changed_book FROM chapters c WHERE c.id=OLD.chapter_id;
            PERFORM rec_invalidate_book_review(changed_book);
        END IF;
        IF TG_OP IN ('UPDATE','INSERT') THEN
            SELECT c.book_id INTO changed_book FROM chapters c WHERE c.id=NEW.chapter_id;
            PERFORM rec_invalidate_book_review(changed_book);
        END IF;
    ELSIF TG_TABLE_NAME='book_cover_slots' THEN
        IF TG_OP IN ('UPDATE','DELETE') THEN
            PERFORM rec_invalidate_book_review(OLD.book_id);
        END IF;
        IF TG_OP IN ('UPDATE','INSERT') THEN
            PERFORM rec_invalidate_book_review(NEW.book_id);
        END IF;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER rec_review_revoke_book_change
    AFTER UPDATE ON books
    FOR EACH ROW EXECUTE FUNCTION rec_invalidate_review_on_content_change();
CREATE TRIGGER rec_review_revoke_chapter_change
    AFTER INSERT OR UPDATE OR DELETE ON chapters
    FOR EACH ROW EXECUTE FUNCTION rec_invalidate_review_on_content_change();
CREATE TRIGGER rec_review_revoke_publication_change
    AFTER INSERT OR UPDATE OR DELETE ON chapter_publications
    FOR EACH ROW EXECUTE FUNCTION rec_invalidate_review_on_content_change();
CREATE TRIGGER rec_review_revoke_cover_change
    AFTER INSERT OR UPDATE OR DELETE ON book_cover_slots
    FOR EACH ROW EXECUTE FUNCTION rec_invalidate_review_on_content_change();
