-- A-05/MOD-B01: retain an immutable audit for real moderator book holds.
-- The author publication flag remains independent of moderation holds.
CREATE TABLE moderation_book_hold_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id > 0),
    book_id BIGINT NOT NULL CHECK (book_id > 0),
    before_hold BOOLEAN NOT NULL,
    after_hold BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT moderation_book_hold_changed CHECK (before_hold <> after_hold)
);
CREATE INDEX moderation_book_hold_events_book_recent
    ON moderation_book_hold_events (book_id, id DESC);
CREATE INDEX moderation_book_hold_events_actor_recent
    ON moderation_book_hold_events (actor_user_id, id DESC);

-- A deleted book/account must not silently delete past moderation evidence.
CREATE FUNCTION moderation_book_hold_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'moderation book hold event is immutable'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER moderation_book_hold_events_immutable
    BEFORE UPDATE OR DELETE ON moderation_book_hold_events
    FOR EACH ROW EXECUTE FUNCTION moderation_book_hold_immutable();
