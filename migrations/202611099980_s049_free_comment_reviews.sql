-- S049: administrator-only manual decisions for reports about currently published
-- free-edition comments. A review is evidence, never an automatic sanction.
CREATE TABLE social_free_comment_review_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    report_id BIGINT NOT NULL UNIQUE
        REFERENCES social_free_comment_reports(id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('reviewed','dismissed')),
    note TEXT NOT NULL CHECK (
        note=btrim(note) AND char_length(note) BETWEEN 1 AND 500
    ),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX social_free_comment_review_events_reviewer
    ON social_free_comment_review_events(reviewer_user_id,decided_at DESC);

CREATE FUNCTION s049_free_comment_review_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'S049 free comment review evidence is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER s049_free_comment_review_immutable_guard
    BEFORE UPDATE OR DELETE ON social_free_comment_review_events
    FOR EACH ROW EXECUTE FUNCTION s049_free_comment_review_immutable();
