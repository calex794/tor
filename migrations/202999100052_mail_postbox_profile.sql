-- MAIL 3/6: exact-intent proof scope for a stopped Postbox candidate.
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
            'mail.postbox.profile.save.v1'
        )
        AND step_up_resource_id>0 AND step_up_revision>0
        AND octet_length(step_up_payload_digest)=32
        AND step_up_issued_at IS NOT NULL
        AND step_up_expires_at>step_up_issued_at
        AND step_up_expires_at<=step_up_issued_at+INTERVAL '5 minutes'
        AND (step_up_used_at IS NULL OR step_up_used_at>=step_up_issued_at)
    )
);

-- A candidate is never an activation. There is deliberately no active flag.
CREATE TABLE mail_postbox_profile (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision>0),
    identity_id TEXT,
    sender TEXT,
    key_version INTEGER CHECK (key_version>0),
    nonce BYTEA,
    ciphertext BYTEA,
    saved_at TIMESTAMPTZ,
    CHECK (
        (identity_id IS NULL AND sender IS NULL AND key_version IS NULL
          AND nonce IS NULL AND ciphertext IS NULL AND saved_at IS NULL)
        OR
        (identity_id IS NOT NULL AND length(identity_id) BETWEEN 1 AND 128
          AND sender IS NOT NULL AND octet_length(sender) BETWEEN 3 AND 254
          AND key_version IS NOT NULL AND nonce IS NOT NULL AND octet_length(nonce)=12
          AND ciphertext IS NOT NULL AND octet_length(ciphertext) BETWEEN 20 AND 2068
          AND saved_at IS NOT NULL)
    )
);
INSERT INTO mail_postbox_profile(singleton) VALUES(true);
CREATE TABLE mail_postbox_profile_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id>0),
    old_revision BIGINT NOT NULL CHECK (old_revision>0),
    new_revision BIGINT NOT NULL UNIQUE CHECK (new_revision=old_revision+1),
    key_version INTEGER NOT NULL CHECK (key_version>0),
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);
CREATE TRIGGER mail_postbox_profile_audit_immutable
BEFORE UPDATE OR DELETE OR TRUNCATE ON mail_postbox_profile_audit
FOR EACH STATEMENT EXECUTE FUNCTION mail_cost_admin_audit_immutable();
