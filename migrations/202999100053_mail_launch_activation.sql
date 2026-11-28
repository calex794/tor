-- MAIL 3/6: distinct exact-intent scope for activation.
ALTER TABLE sessions DROP CONSTRAINT sessions_step_up_proof_shape;
ALTER TABLE sessions ADD CONSTRAINT sessions_step_up_proof_shape CHECK (
    (
        step_up_proof_hash IS NULL AND step_up_scope IS NULL
        AND step_up_resource_id IS NULL AND step_up_revision IS NULL
        AND step_up_payload_digest IS NULL AND step_up_issued_at IS NULL
        AND step_up_expires_at IS NULL AND step_up_used_at IS NULL
    ) OR (
        step_up_proof_hash IS NOT NULL AND octet_length(step_up_proof_hash)=32
        AND step_up_scope IN (
            'finance.refund.approve.v1',
            'book.owner.transfer.v1',
            'book.revenue.shares.v1',
            'finance.payout.post.v1',
            'storage.s3.connection.save.v1',
            'mail.postbox.profile.save.v1',
            'mail.launch.activate.v1'
        )
        AND step_up_resource_id>0 AND step_up_revision>0
        AND octet_length(step_up_payload_digest)=32
        AND step_up_issued_at IS NOT NULL
        AND step_up_expires_at>step_up_issued_at
        AND step_up_expires_at<=step_up_issued_at+INTERVAL '5 minutes'
        AND (step_up_used_at IS NULL OR step_up_used_at>=step_up_issued_at)
    )
);

-- Immutable receipt of one explicit launch; a fresh process is still stopped.
CREATE TABLE mail_launch_activation_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id>0),
    old_revision BIGINT NOT NULL CHECK (old_revision>0),
    new_revision BIGINT NOT NULL UNIQUE CHECK (new_revision=old_revision+1),
    mode TEXT NOT NULL CHECK (mode IN ('dispatch','operator_test')),
    test_user_id BIGINT,
    month_start DATE NOT NULL CHECK (EXTRACT(DAY FROM month_start)=1),
    profile_digest BYTEA NOT NULL CHECK (octet_length(profile_digest)=32),
    evidence_sha256 BYTEA NOT NULL CHECK (octet_length(evidence_sha256)=32),
    intent_digest BYTEA NOT NULL UNIQUE CHECK (octet_length(intent_digest)=32),
    postmaster_epoch BIGINT NOT NULL CHECK (postmaster_epoch>0),
    database_oid BIGINT NOT NULL CHECK (database_oid>0),
    reserved_attempts BIGINT NOT NULL CHECK (reserved_attempts>=0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK ((mode='dispatch' AND test_user_id IS NULL)
        OR (mode='operator_test' AND test_user_id IS NOT NULL AND test_user_id=actor_user_id))
);
CREATE TRIGGER mail_launch_activation_audit_immutable
BEFORE UPDATE OR DELETE OR TRUNCATE ON mail_launch_activation_audit
FOR EACH STATEMENT EXECUTE FUNCTION mail_cost_admin_audit_immutable();
