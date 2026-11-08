-- E-08/S057: public book rooms, distinct from private group/direct mail.
-- One canonical room per Book; book visibility is still checked per action.
CREATE TABLE social_book_rooms (
    book_id BIGINT PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
    created_by BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE social_book_room_members (
    book_id BIGINT NOT NULL REFERENCES social_book_rooms(book_id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','left')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    left_at TIMESTAMPTZ,
    PRIMARY KEY(book_id,user_id),
    CHECK ((status='active' AND left_at IS NULL)
       OR (status='left' AND left_at IS NOT NULL))
);
CREATE INDEX social_book_room_members_active
    ON social_book_room_members(user_id,book_id DESC) WHERE status='active';

CREATE TABLE social_book_room_messages (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK(id > 0),
    book_id BIGINT NOT NULL REFERENCES social_book_rooms(book_id) ON DELETE CASCADE,
    sender_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body TEXT NOT NULL CHECK(body <> '' AND body=btrim(body)),
    command_key TEXT NOT NULL CHECK(command_key ~ '^[A-Za-z0-9_-]{43}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(sender_id,command_key)
);
CREATE INDEX social_book_room_messages_page
    ON social_book_room_messages(book_id,id DESC);

-- Server-side writes and row locks are the primary ACL. This trigger also
-- refuses forged direct SQL inserts and edits to existing message content.
CREATE FUNCTION social_book_room_message_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN
        RAISE EXCEPTION 'book room messages are immutable'
            USING ERRCODE='23514';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM social_book_room_members m
        JOIN books b ON b.id=m.book_id
        WHERE m.book_id=NEW.book_id AND m.user_id=NEW.sender_id
          AND m.status='active' AND b.visibility='public'
          AND NOT b.admin_hold AND NOT b.is_deleted
    ) THEN
        RAISE EXCEPTION 'sender is not an active public book room member'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER social_book_room_message_identity
    BEFORE INSERT OR UPDATE ON social_book_room_messages
    FOR EACH ROW EXECUTE FUNCTION social_book_room_message_guard();
