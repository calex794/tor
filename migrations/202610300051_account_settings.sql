-- A-06: private, revisioned settings. Adult opt-in remains the existing user fact.
CREATE TABLE account_settings (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    handle TEXT UNIQUE CHECK (handle ~ '^[a-z0-9_.]{3,32}$'),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
