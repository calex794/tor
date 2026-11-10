-- E/S018: proof-bound, reader-owned opt-in for future external email content.
-- No default email consent, no channel or delivery adapter is enabled here.
-- This table is not a queue and does not reuse password-reset credentials.
CREATE TABLE reader_notification_email_proofs (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    email_sha256 BYTEA NOT NULL CHECK (octet_length(email_sha256)=32),
    challenge_hash BYTEA CHECK (challenge_hash IS NULL OR octet_length(challenge_hash)=32),
    challenge_expires_at TIMESTAMPTZ,
    requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    verified_at TIMESTAMPTZ,
    opted_in_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    CONSTRAINT s018_email_proof_pair CHECK (
        (challenge_hash IS NULL) = (challenge_expires_at IS NULL)
    ),
    CONSTRAINT s018_email_consent_verified CHECK (
        opted_in_at IS NULL OR verified_at IS NOT NULL
    ),
    CONSTRAINT s018_email_consent_revocation CHECK (
        revoked_at IS NULL OR opted_in_at IS NULL
    )
);
CREATE UNIQUE INDEX reader_notification_email_proofs_challenge_uq
    ON reader_notification_email_proofs (challenge_hash)
    WHERE challenge_hash IS NOT NULL;
