-- S027 / B: recipient-bound expiring Team invites are not Book or paid entitlements.
-- Add-only migration for disposable PostgreSQL CI; no production rollout.
CREATE TABLE translation_team_invites (
    token_digest BYTEA PRIMARY KEY CHECK (octet_length(token_digest) = 32),
    team_id BIGINT NOT NULL REFERENCES translation_teams(id) ON DELETE RESTRICT,
    invitee_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    inviter_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'declined', 'expired')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '7 days'),
    decided_at TIMESTAMPTZ,
    CHECK (expires_at > created_at),
    CHECK (invitee_user_id <> inviter_user_id),
    CHECK ((status = 'pending' AND decided_at IS NULL)
        OR (status <> 'pending' AND decided_at IS NOT NULL))
);
CREATE UNIQUE INDEX translation_team_invites_one_pending
    ON translation_team_invites(team_id, invitee_user_id)
    WHERE status = 'pending';
CREATE INDEX translation_team_invites_recipient_pending
    ON translation_team_invites(invitee_user_id, expires_at)
    WHERE status = 'pending';

CREATE TABLE translation_team_members (
    team_id BIGINT NOT NULL REFERENCES translation_teams(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    invite_digest BYTEA NOT NULL REFERENCES translation_team_invites(token_digest) ON DELETE RESTRICT,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id)
);
CREATE INDEX translation_team_members_user
    ON translation_team_members(user_id, team_id);
