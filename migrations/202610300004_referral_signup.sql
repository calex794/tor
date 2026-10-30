-- F-09/S052: durable referral capture and registration attribution.
-- No wallet, balance, paid entitlement, or advertising attribution is created.
CREATE TABLE referral_codes (
    code TEXT PRIMARY KEY
        CHECK (char_length(code) BETWEEN 6 AND 64
            AND code ~ '^[A-Za-z0-9_-]+$'),
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    program_type TEXT NOT NULL CHECK (program_type IN ('reader_spend', 'creator_income')),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled', 'blocked')),
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    alias TEXT,
    alias_path_prefix TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((alias IS NULL) = (alias_path_prefix IS NULL)),
    CHECK (alias IS NULL OR (char_length(alias) BETWEEN 3 AND 64
        AND alias ~ '^[a-z0-9][a-z0-9-]+$')),
    CHECK (alias_path_prefix IS NULL OR alias_path_prefix IN ('go', 'promo'))
);
CREATE UNIQUE INDEX referral_codes_default_owner
    ON referral_codes(owner_user_id, program_type) WHERE is_default;
CREATE UNIQUE INDEX referral_codes_alias
    ON referral_codes(alias_path_prefix, alias) WHERE alias IS NOT NULL;
CREATE INDEX referral_codes_by_owner ON referral_codes(owner_user_id, program_type);

CREATE TABLE referral_captures (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    guest_hash BYTEA NOT NULL CHECK (octet_length(guest_hash) = 32),
    referral_code TEXT NOT NULL REFERENCES referral_codes(code) ON DELETE RESTRICT,
    source_kind TEXT NOT NULL CHECK (source_kind IN ('direct_ref_link', 'book_ref_query')),
    source_path TEXT NOT NULL CHECK (char_length(source_path) BETWEEN 1 AND 256),
    captured_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '30 days'),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'consumed', 'expired', 'blocked')),
    CHECK (expires_at > captured_at)
);
CREATE UNIQUE INDEX referral_captures_one_active_guest
    ON referral_captures(guest_hash) WHERE status = 'active';
CREATE INDEX referral_captures_expiration
    ON referral_captures(expires_at) WHERE status = 'active';

-- A-owned durable registration handoff, written in the same transaction as
-- users and the first session; F consumes it under a row lock.
CREATE TABLE account_registration_referral_outbox (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE RESTRICT,
    guest_hash BYTEA CHECK (guest_hash IS NULL OR octet_length(guest_hash) = 32),
    manual_code TEXT CHECK (manual_code IS NULL OR (char_length(manual_code) BETWEEN 6 AND 64
        AND manual_code ~ '^[A-Za-z0-9_-]+$')),
    -- An immutable decision from the account/session transaction. Never
    -- recompute a live code status when recovering a committed registration.
    -- Legacy/test handoffs without a decision retain the old recovery path.
    decided_at_registration BOOLEAN NOT NULL DEFAULT FALSE,
    decided_referral_code TEXT REFERENCES referral_codes(code) ON DELETE RESTRICT,
    decided_referrer_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    decided_program_type TEXT
        CHECK (decided_program_type IN ('reader_spend','creator_income')),
    decided_capture_id BIGINT REFERENCES referral_captures(id) ON DELETE RESTRICT,
    decided_source TEXT
        CHECK (decided_source IN ('guest_capture','manual_code_at_registration')),
    registered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ,
    result TEXT CHECK (result IS NULL OR result IN ('attributed', 'unattributed')),
    CHECK (guest_hash IS NOT NULL OR manual_code IS NOT NULL),
    CHECK ((processed_at IS NULL) = (result IS NULL)),
    CHECK (
        (decided_referral_code IS NULL AND decided_referrer_user_id IS NULL
            AND decided_program_type IS NULL AND decided_capture_id IS NULL
            AND decided_source IS NULL)
        OR
        (decided_at_registration AND decided_referral_code IS NOT NULL
            AND decided_referrer_user_id IS NOT NULL
            AND decided_program_type IS NOT NULL AND decided_source IS NOT NULL)
    ),
    CHECK (decided_capture_id IS NULL OR decided_source='guest_capture')
);
CREATE INDEX account_registration_referral_outbox_pending
    ON account_registration_referral_outbox(registered_at, user_id)
    WHERE processed_at IS NULL;

CREATE TABLE referral_attributions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    program_type TEXT NOT NULL CHECK (program_type IN ('reader_spend', 'creator_income')),
    subject_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    referrer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    referral_code TEXT NOT NULL REFERENCES referral_codes(code) ON DELETE RESTRICT,
    source_capture_id BIGINT REFERENCES referral_captures(id) ON DELETE RESTRICT,
    attribution_source TEXT NOT NULL
        CHECK (attribution_source IN ('guest_capture', 'manual_code_at_registration')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'under_review', 'blocked', 'expired')),
    attributed_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ,
    CHECK (subject_user_id <> referrer_user_id),
    UNIQUE (program_type, subject_user_id)
);
CREATE INDEX referral_attributions_by_referrer
    ON referral_attributions(referrer_user_id, program_type, attributed_at DESC);

-- Runtime role receives only ordinary DML through tor_migrator default grants.
-- Real-money operations remain explicitly outside this migration.
