-- S049: complaint on a visible comment of the exact currently published free edition.
-- A report is evidence only, never an automatic chapter hold or comment hide.
CREATE TABLE social_free_comment_reports (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    comment_id BIGINT NOT NULL REFERENCES discussion_comments(id) ON DELETE RESTRICT,
    reporter_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL CHECK (reason IN ('spam','abuse','misleading','other')),
    details TEXT NOT NULL CHECK (char_length(details)<=500),
    command_key TEXT NOT NULL CHECK (command_key ~ '^[A-Za-z0-9_-]{43}$'),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','reviewed','dismissed')),
    reported_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY (chapter_id,version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE RESTRICT,
    UNIQUE (reporter_user_id,command_key)
);
CREATE UNIQUE INDEX social_free_comment_reports_one_pending
    ON social_free_comment_reports(reporter_user_id,comment_id)
    WHERE state='pending';
CREATE INDEX social_free_comment_reports_pending
    ON social_free_comment_reports(reported_at,id) WHERE state='pending';
CREATE INDEX social_free_comment_reports_quota
    ON social_free_comment_reports(reporter_user_id,reported_at DESC);

CREATE FUNCTION s049_free_comment_target_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM discussion_comments c
        WHERE c.id=NEW.comment_id AND c.chapter_id=NEW.chapter_id
          AND c.version_id=NEW.version_id AND c.book_id IS NULL
    ) THEN
        RAISE EXCEPTION 'S049 free comment report requires exact chapter edition'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER s049_free_comment_target_guard
    BEFORE INSERT OR UPDATE OF chapter_id,version_id,comment_id
    ON social_free_comment_reports
    FOR EACH ROW EXECUTE FUNCTION s049_free_comment_target_guard();
