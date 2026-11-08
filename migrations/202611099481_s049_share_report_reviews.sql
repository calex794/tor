-- S049 first slice: auditable site-administrator review of S017 share complaints.
-- Complaint review does not grant a Book, chapter, payment or moderation power.
CREATE TABLE social_book_share_report_reviews (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    share_id BIGINT NOT NULL UNIQUE
        REFERENCES social_book_share_reports(share_id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('reviewed','dismissed')),
    note TEXT NOT NULL CHECK (char_length(note) BETWEEN 1 AND 500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX social_book_share_report_reviews_actor
    ON social_book_share_report_reviews(reviewer_user_id,created_at DESC);

CREATE FUNCTION s049_report_review_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'S049 report review evidence is immutable'
        USING ERRCODE = '23514';
END;
$$;
CREATE TRIGGER s049_report_review_immutable_guard
    BEFORE UPDATE OR DELETE ON social_book_share_report_reviews
    FOR EACH ROW EXECUTE FUNCTION s049_report_review_immutable();
