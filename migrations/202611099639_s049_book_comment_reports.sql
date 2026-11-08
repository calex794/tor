-- S049: reports about visible public Book comments, not paid or free chapters.
-- Evidence and decisions do not alter comment visibility or any reader entitlement.
CREATE TABLE social_book_comment_reports (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    comment_id BIGINT NOT NULL REFERENCES discussion_comments(id) ON DELETE RESTRICT,
    reporter_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    body_snapshot TEXT NOT NULL CHECK (char_length(body_snapshot) BETWEEN 1 AND 2000),
    reason TEXT NOT NULL CHECK (reason IN ('spam','abuse','spoiler','other')),
    details TEXT NOT NULL CHECK (char_length(details) <= 500),
    command_key TEXT NOT NULL CHECK (command_key ~ '^[A-Za-z0-9_-]{43}$'),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','reviewed','dismissed')),
    reported_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (reporter_user_id, command_key)
);
CREATE UNIQUE INDEX social_book_comment_reports_one_pending
    ON social_book_comment_reports(reporter_user_id, comment_id)
    WHERE state='pending';
CREATE INDEX social_book_comment_reports_pending
    ON social_book_comment_reports(reported_at,id) WHERE state='pending';
CREATE INDEX social_book_comment_reports_rate
    ON social_book_comment_reports(reporter_user_id,reported_at DESC);

CREATE TABLE social_book_comment_review_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    report_id BIGINT NOT NULL UNIQUE
        REFERENCES social_book_comment_reports(id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    decision TEXT NOT NULL CHECK (decision IN ('reviewed','dismissed')),
    note TEXT NOT NULL CHECK (char_length(note) BETWEEN 1 AND 500),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX social_book_comment_review_events_reviewer
    ON social_book_comment_review_events(reviewer_user_id,decided_at DESC);

CREATE FUNCTION s049_book_comment_review_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'S049 Book comment review evidence is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER s049_book_comment_review_immutable_guard
    BEFORE UPDATE OR DELETE ON social_book_comment_review_events
    FOR EACH ROW EXECUTE FUNCTION s049_book_comment_review_immutable();

-- A report may change state once; saved target evidence must not be rewritten.
CREATE FUNCTION s049_book_comment_report_evidence_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(OLD.book_id, OLD.comment_id, OLD.reporter_user_id, OLD.body_snapshot,
           OLD.reason, OLD.details, OLD.command_key, OLD.reported_at)
       IS DISTINCT FROM
       ROW(NEW.book_id, NEW.comment_id, NEW.reporter_user_id, NEW.body_snapshot,
           NEW.reason, NEW.details, NEW.command_key, NEW.reported_at)
    THEN
        RAISE EXCEPTION 'S049 Book comment report evidence is immutable'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER s049_book_comment_report_evidence_immutable_guard
    BEFORE UPDATE ON social_book_comment_reports
    FOR EACH ROW EXECUTE FUNCTION s049_book_comment_report_evidence_immutable();