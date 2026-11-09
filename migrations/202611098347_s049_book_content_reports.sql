-- S049 / Book content complaints: separate from S017 friend recommendations.
-- No report, review or state transition changes reader entitlements or Book visibility.
CREATE TABLE social_book_content_reports (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    reporter_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL CHECK (reason IN ('spam','abuse','misleading','other')),
    details TEXT NOT NULL CHECK (char_length(details) <= 500),
    command_key TEXT NOT NULL CHECK (command_key ~ '^[A-Za-z0-9_-]{43}$'),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','reviewed','dismissed')),
    reported_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (reporter_user_id,command_key)
);
CREATE UNIQUE INDEX social_book_content_reports_one_pending
    ON social_book_content_reports(reporter_user_id,book_id) WHERE state='pending';
CREATE INDEX social_book_content_reports_pending
    ON social_book_content_reports(reported_at,id) WHERE state='pending';
CREATE INDEX social_book_content_reports_rate
    ON social_book_content_reports(reporter_user_id,reported_at DESC);

CREATE TABLE social_book_content_review_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    report_id BIGINT NOT NULL UNIQUE
        REFERENCES social_book_content_reports(id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('reviewed','dismissed')),
    note TEXT NOT NULL CHECK (char_length(note) BETWEEN 1 AND 500),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX social_book_content_review_events_reviewer
    ON social_book_content_review_events(reviewer_user_id,decided_at DESC);

CREATE FUNCTION s049_book_content_review_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'S049 Book report review evidence is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER s049_book_content_review_immutable_guard
    BEFORE UPDATE OR DELETE ON social_book_content_review_events
    FOR EACH ROW EXECUTE FUNCTION s049_book_content_review_immutable();
