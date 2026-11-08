-- E-08/S057: group conversations are a separate aggregate from immutable
-- direct two-person mail. No reinterpreting user_low/user_high or direct IDs.
CREATE TABLE social_group_conversations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    created_by BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (
        char_length(title) BETWEEN 1 AND 120 AND title = btrim(title)
    ),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE social_group_members (
    group_id BIGINT NOT NULL REFERENCES social_group_conversations(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('creator','member')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active','left','removed','banned')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    left_at TIMESTAMPTZ,
    PRIMARY KEY (group_id,user_id),
    CHECK ((status='active' AND left_at IS NULL)
       OR (status<>'active' AND left_at IS NOT NULL))
);
CREATE INDEX social_group_members_actor_active
    ON social_group_members(user_id,group_id DESC) WHERE status='active';

CREATE TABLE social_group_messages (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    group_id BIGINT NOT NULL REFERENCES social_group_conversations(id) ON DELETE CASCADE,
    sender_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body TEXT NOT NULL CHECK (body <> '' AND body=btrim(body)),
    command_key TEXT NOT NULL CHECK (command_key ~ '^[A-Za-z0-9_-]{43}$'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(sender_id,command_key)
);
CREATE INDEX social_group_messages_page
    ON social_group_messages(group_id,id DESC);

-- Belt-and-suspenders protection for inserts that bypass the E adapter.
-- The adapter also locks its member row through the insertion transaction
-- to serialize sends with membership revocation.
CREATE FUNCTION social_group_message_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN
        RAISE EXCEPTION 'group mail content and identity are immutable'
            USING ERRCODE='23514';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM social_group_members m
        WHERE m.group_id=NEW.group_id AND m.user_id=NEW.sender_id
          AND m.status='active'
    ) THEN
        RAISE EXCEPTION 'sender is not an active group participant'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER social_group_message_identity
    BEFORE INSERT OR UPDATE ON social_group_messages
    FOR EACH ROW EXECUTE FUNCTION social_group_message_guard();
