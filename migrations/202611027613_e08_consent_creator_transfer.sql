-- E-08: proposals to hand off group management require recipient consent.
-- This migration is append-only. The existing creator and members stay put
-- until the addressed active member explicitly accepts the offer.
CREATE TABLE social_group_creator_transfers (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    group_id BIGINT NOT NULL REFERENCES social_group_conversations(id) ON DELETE CASCADE,
    from_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','accepted','declined','revoked','expired')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp() + INTERVAL '7 days'),
    resolved_at TIMESTAMPTZ,
    CHECK (from_user_id <> to_user_id),
    CHECK (expires_at > created_at),
    CHECK ((state='pending' AND resolved_at IS NULL)
        OR (state<>'pending' AND resolved_at IS NOT NULL))
);
CREATE UNIQUE INDEX social_group_one_pending_creator_transfer
    ON social_group_creator_transfers(group_id) WHERE state='pending';
CREATE INDEX social_group_creator_transfers_inbox
    ON social_group_creator_transfers(to_user_id,id DESC) WHERE state='pending';
CREATE INDEX social_group_creator_transfers_sent
    ON social_group_creator_transfers(from_user_id,id DESC) WHERE state='pending';
