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


-- Browser-held opaque proof; Telegram receives only a comparison code and
-- an approve/deny callback, never a bearer reset token.
-- For unknown or ineligible identities a decoy row is stored with user_id=NULL,
-- preserving the same user-facing challenge flow without revealing an account.
CREATE TABLE account_telegram_recovery_requests (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT REFERENCES users(id) ON DELETE CASCADE,
    telegram_user_id BIGINT,
    private_chat_id BIGINT,
    credential_fingerprint BYTEA,
    browser_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(browser_digest) = 32),
    match_code TEXT NOT NULL CHECK (match_code ~ '^[A-Za-z0-9_-]{6}$'),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'denied', 'used')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now()+INTERVAL '5 minutes',
    notified_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    CHECK (
      (user_id IS NULL AND telegram_user_id IS NULL AND private_chat_id IS NULL
       AND credential_fingerprint IS NULL)
      OR
      (user_id IS NOT NULL AND telegram_user_id IS NOT NULL
       AND private_chat_id IS NOT NULL AND credential_fingerprint IS NOT NULL
       AND telegram_user_id>0 AND private_chat_id>0
       AND octet_length(credential_fingerprint)=32)
    )
);
CREATE INDEX account_telegram_recovery_outbox
    ON account_telegram_recovery_requests(id)
    WHERE user_id IS NOT NULL AND status='pending' AND notified_at IS NULL;
CREATE INDEX account_telegram_recovery_expiry
    ON account_telegram_recovery_requests(expires_at);


-- Exactly one active long-polling owner for a Telegram bot id across TOR nodes.
-- A fenced, expiring lease uses database time, not node wall clocks. Every
-- renewed owner must present the original digest and unchanged epoch.
CREATE TABLE account_telegram_poller_leases (
    bot_id BIGINT PRIMARY KEY CHECK (bot_id > 0),
    owner_digest BYTEA NOT NULL CHECK (octet_length(owner_digest) = 32),
    lease_epoch BIGINT NOT NULL DEFAULT 1 CHECK (lease_epoch > 0),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX account_telegram_poller_leases_expiry
    ON account_telegram_poller_leases(expires_at);
