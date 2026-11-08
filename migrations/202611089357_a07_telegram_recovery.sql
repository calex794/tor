-- A-07 / S004: optional Telegram-bound recovery, default-off at runtime.
-- New additive version; never apply to a retained database as part of this PR.
ALTER TABLE password_reset_tokens
    ADD COLUMN source TEXT NOT NULL DEFAULT 'email'
    CHECK (source IN ('email', 'telegram', 'backup'));

CREATE TABLE account_telegram_bindings (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    telegram_user_id BIGINT NOT NULL UNIQUE CHECK (telegram_user_id > 0),
    private_chat_id BIGINT NOT NULL UNIQUE CHECK (private_chat_id > 0),
    verified_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE account_telegram_link_intents (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    session_digest BYTEA NOT NULL CHECK (octet_length(session_digest) = 32),
    credential_fingerprint BYTEA NOT NULL CHECK (octet_length(credential_fingerprint) = 32),
    candidate_telegram_user_id BIGINT CHECK (candidate_telegram_user_id > 0),
    candidate_private_chat_id BIGINT CHECK (candidate_private_chat_id > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '5 minutes',
    CHECK ((candidate_telegram_user_id IS NULL) = (candidate_private_chat_id IS NULL)),
    CHECK (expires_at > created_at)
);
CREATE INDEX account_telegram_link_intents_expiry ON account_telegram_link_intents(expires_at);

CREATE TABLE account_recovery_codes (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BYTEA NOT NULL CHECK (octet_length(code_hash) = 32),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    consumed_at TIMESTAMPTZ,
    PRIMARY KEY(user_id, code_hash)
);
CREATE INDEX account_recovery_codes_active ON account_recovery_codes(user_id)
    WHERE consumed_at IS NULL;

-- Telegram retries the same update after a failed webhook acknowledgement.
CREATE TABLE account_telegram_bot_updates (
    update_id BIGINT PRIMARY KEY,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX account_telegram_bot_updates_expiry ON account_telegram_bot_updates(received_at);


-- Private audit. Never persist reset secrets, session tokens, or usernames.
CREATE TABLE account_telegram_binding_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    telegram_user_id BIGINT NOT NULL CHECK (telegram_user_id > 0),
    action TEXT NOT NULL CHECK (action IN ('linked', 'unlinked')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX account_telegram_binding_events_by_user
    ON account_telegram_binding_events(user_id, created_at DESC);
CREATE INDEX account_telegram_binding_events_expiry
    ON account_telegram_binding_events(created_at);
