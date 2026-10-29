-- D-05: idempotent owner-only Layer creation from an authenticated form.
-- Additive migration: never rewrite frozen paid terms or historical migrations.
ALTER TABLE commerce_layers
    ADD COLUMN create_command_key TEXT,
    ADD COLUMN create_command_title TEXT;

-- NULL keeps existing internal/test creation APIs valid. A real HTTP create
-- always supplies a canonical 32-byte, unpadded base64url command token.
ALTER TABLE commerce_layers
    ADD CONSTRAINT commerce_layer_create_command_format
    CHECK (
        (create_command_key IS NULL AND create_command_title IS NULL)
        OR (
            create_command_key IS NOT NULL
            AND create_command_title IS NOT NULL
            AND char_length(create_command_title) BETWEEN 1 AND 120
            AND char_length(create_command_key) = 43
            AND create_command_key ~ '^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]
    );

-- Frozen request title allows replay after the visible Layer name is edited.
-- The unique index is the concurrency arbiter, not a read-before-write check.
-- A command key belongs to one authenticated owner and one immutable title.
ALTER TABLE commerce_layers
    ADD CONSTRAINT commerce_layer_create_owner_command_unique
    UNIQUE (owner_user_id, create_command_key);

        )
    );

-- The unique index is the concurrency arbiter, not a read-before-write check.
-- A command key belongs to one authenticated owner and one immutable title.
ALTER TABLE commerce_layers
    ADD CONSTRAINT commerce_layer_create_owner_command_unique
    UNIQUE (owner_user_id, create_command_key);

-- The visible title may change, but the original creation command identity
-- must not. A later title edit must not reinterpret old retries as conflicts.
CREATE FUNCTION commerce_layer_creation_identity_immutable()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.create_command_key IS DISTINCT FROM NEW.create_command_key
       OR OLD.create_command_title IS DISTINCT FROM NEW.create_command_title THEN
        RAISE EXCEPTION 'Layer creation command identity cannot change'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER commerce_layer_creation_identity_guard
BEFORE UPDATE ON commerce_layers
FOR EACH ROW EXECUTE FUNCTION commerce_layer_creation_identity_immutable();
