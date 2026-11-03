-- A-05/MOD-C01: immutable decisions on chapter moderation holds.
-- These records are not purchases, chapter versions or author publication.
CREATE TABLE moderation_chapter_hold_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id > 0),
    book_id BIGINT NOT NULL CHECK (book_id > 0),
    chapter_id BIGINT NOT NULL CHECK (chapter_id > 0),
    before_hold BOOLEAN NOT NULL,
    after_hold BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT moderation_chapter_hold_changed CHECK (before_hold <> after_hold)
);
CREATE INDEX moderation_chapter_hold_events_chapter_recent
    ON moderation_chapter_hold_events (chapter_id, id DESC);
CREATE INDEX moderation_chapter_hold_events_actor_recent
    ON moderation_chapter_hold_events (actor_user_id, id DESC);

-- Deliberately no cascading FK to chapter/book/user: deletion must not
-- silently erase the security record of a moderator's decision.
CREATE FUNCTION moderation_chapter_hold_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'moderation chapter hold event is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER moderation_chapter_hold_events_immutable
    BEFORE UPDATE OR DELETE ON moderation_chapter_hold_events
    FOR EACH ROW EXECUTE FUNCTION moderation_chapter_hold_immutable();
