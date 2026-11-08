-- E-08: explicit, expiring group invitations. No membership until acceptance.
-- Historical left/removed membership can only be reactivated by the invited actor.
CREATE TABLE social_group_invitations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    group_id BIGINT NOT NULL REFERENCES social_group_conversations(id) ON DELETE CASCADE,
    invited_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    invited_by BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','accepted','declined','revoked','expired')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp() + INTERVAL '7 days'),
    resolved_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    CHECK ((state='pending' AND resolved_at IS NULL)
        OR (state<>'pending' AND resolved_at IS NOT NULL))
);
CREATE UNIQUE INDEX social_group_one_pending_invitation
    ON social_group_invitations(group_id,invited_user_id) WHERE state='pending';
CREATE INDEX social_group_invitations_inbox
    ON social_group_invitations(invited_user_id,id DESC) WHERE state='pending';
CREATE INDEX social_group_invitations_group_pending
    ON social_group_invitations(group_id,id DESC) WHERE state='pending';
