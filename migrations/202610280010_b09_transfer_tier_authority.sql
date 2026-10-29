-- B-09d / A+B+I: trusted Book and recipient transfer eligibility source.
-- Legacy Books are deliberately NULL: importing an unknown age tier as
-- 'basic' could make a previously restricted Book transferable to anyone.
ALTER TABLE books
    ADD COLUMN access_level TEXT
        CHECK (access_level IN ('basic','adult','pro_adult'));

-- Adult is an explicit user choice, not an inferred property of registration.
-- A pro_adult grant outranks adult_enabled without changing that user flag.
ALTER TABLE users
    ADD COLUMN adult_enabled BOOLEAN NOT NULL DEFAULT FALSE;

-- Persistent A-owned provenance for elevated user level. Only trusted
-- moderation/invitation services may create/revoke grants; never a reader
-- request field. One canonical grant row per user+level, revocation is
-- represented by is_active=FALSE rather than an implicit absence of history.
CREATE TABLE user_level_grants (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    level TEXT NOT NULL CHECK (level='pro_adult'),
    source TEXT NOT NULL CHECK (source IN ('moderation','invitation')),
    source_ref_id BIGINT,
    granted_by_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id,level),
    CONSTRAINT pro_adult_invitation_provenance
        CHECK (source<>'invitation' OR source_ref_id IS NOT NULL)
);
CREATE UNIQUE INDEX user_level_grants_invitation_once
    ON user_level_grants(source_ref_id)
    WHERE source='invitation';
CREATE INDEX user_level_grants_active
    ON user_level_grants(user_id)
    WHERE is_active;

-- No automatic elevation/backfill and no browser grant endpoint. Historical
-- imported Book level must be attested separately by a trusted migration.
