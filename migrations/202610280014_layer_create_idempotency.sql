-- D-05: owner-scoped idempotent Layer creation via an authenticated form.
-- Additive only: never rewrite financial terms or historical SQLx migrations.
ALTER TABLE commerce_layers
    ADD COLUMN create_command_key TEXT,
    ADD COLUMN create_command_title TEXT;

-- Old service-only Layer creation keeps both optional fields NULL.
-- HTTP creation uses a canonical 32-byte unpadded base64url token.
-- Explicit non-nullness matters: PostgreSQL CHECK accepts UNKNOWN.
ALTER TABLE commerce_layers
    ADD CONSTRAINT commerce_layer_create_command_format
    CHECK (
        (create_command_key IS NULL AND create_command_title IS NULL)
        OR (
            create_command_key IS NOT NULL
            AND create_command_title IS NOT NULL
            AND char_length(create_command_title) BETWEEN 1 AND 120
            AND char_length(create_command_key) = 43
            AND create_command_key ~ '^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$'
        )
    );

-- PG unique conflict arbitration serializes concurrent same-owner retries.
ALTER TABLE commerce_layers
    ADD CONSTRAINT commerce_layer_create_owner_command_unique
    UNIQUE (owner_user_id, create_command_key);

-- The displayed Layer title can change, but the creation command and its
-- canonical original title cannot be rewritten after insertion.
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
