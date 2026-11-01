-- D-06: scoped one-time step-up proof for sensitive direct refund approval.
-- Reuse the existing opaque session row; do not introduce a second auth store.
ALTER TABLE sessions
    ADD COLUMN step_up_proof_hash BYTEA,
    ADD COLUMN step_up_scope TEXT,
    ADD COLUMN step_up_resource_id BIGINT,
    ADD COLUMN step_up_revision BIGINT,
    ADD COLUMN step_up_payload_digest BYTEA,
    ADD COLUMN step_up_issued_at TIMESTAMPTZ,
    ADD COLUMN step_up_expires_at TIMESTAMPTZ,
    ADD COLUMN step_up_used_at TIMESTAMPTZ;

ALTER TABLE sessions
    ADD CONSTRAINT sessions_step_up_proof_shape CHECK (
        (
            step_up_proof_hash IS NULL
            AND step_up_scope IS NULL
            AND step_up_resource_id IS NULL
            AND step_up_revision IS NULL
            AND step_up_payload_digest IS NULL
            AND step_up_issued_at IS NULL
            AND step_up_expires_at IS NULL
            AND step_up_used_at IS NULL
        )
        OR
        (
            step_up_proof_hash IS NOT NULL
            AND octet_length(step_up_proof_hash) = 32
            AND step_up_scope = 'finance.refund.approve.v1'
            AND step_up_resource_id > 0
            AND step_up_revision > 0
            AND octet_length(step_up_payload_digest) = 32
            AND step_up_issued_at IS NOT NULL
            AND step_up_expires_at > step_up_issued_at
            AND step_up_expires_at <= step_up_issued_at + INTERVAL '5 minutes'
            AND (step_up_used_at IS NULL OR step_up_used_at >= step_up_issued_at)
        )
    );

COMMENT ON COLUMN sessions.step_up_proof_hash IS
    'SHA-256 of a random one-time proof; raw proof is never persisted.';
COMMENT ON COLUMN sessions.step_up_scope IS
    'Exact sensitive action authorized by this session proof.';
COMMENT ON COLUMN sessions.step_up_resource_id IS
    'Refund request ID bound to the proof.';
COMMENT ON COLUMN sessions.step_up_revision IS
    'Expected refund request revision bound to the proof.';
COMMENT ON COLUMN sessions.step_up_payload_digest IS
    'SHA-256 of actor, action scope, request, revision and normalized reason.';
COMMENT ON COLUMN sessions.step_up_issued_at IS
    'Database timestamp at which fresh password re-auth was accepted.';
COMMENT ON COLUMN sessions.step_up_expires_at IS
    'Short proof expiry; the application caps it at five minutes.';
COMMENT ON COLUMN sessions.step_up_used_at IS
    'Set in the same transaction as the protected refund decision.';
