-- S049 audit hardening: submitted appeal evidence must not disappear before review.
-- Additive, non-destructive protection for all three accepted appeal workflows.
-- The decision tables already reject UPDATE/DELETE; the original appeal rows
-- permit the one-way pending -> denied/restored transition but never DELETE.
CREATE FUNCTION s049_appeal_submission_delete_rejected()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'S049 appeal submission evidence is immutable'
        USING ERRCODE='23514';
END; $$;

CREATE TRIGGER s049_book_appeal_submission_no_delete
    BEFORE DELETE ON social_book_comment_appeals
    FOR EACH ROW EXECUTE FUNCTION s049_appeal_submission_delete_rejected();
CREATE TRIGGER s049_paid_appeal_submission_no_delete
    BEFORE DELETE ON social_paid_comment_appeals
    FOR EACH ROW EXECUTE FUNCTION s049_appeal_submission_delete_rejected();
CREATE TRIGGER s049_free_appeal_submission_no_delete
    BEFORE DELETE ON social_free_comment_appeals
    FOR EACH ROW EXECUTE FUNCTION s049_appeal_submission_delete_rejected();
