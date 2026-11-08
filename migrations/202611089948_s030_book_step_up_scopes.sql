-- S030: reuse A's single-session one-time proof for Book owner transfer and
-- creator-share revision. Do not modify any historical migration checksum.
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
            'book.revenue.shares.v1'
        )
        AND step_up_resource_id>0 AND step_up_revision>0
        AND octet_length(step_up_payload_digest)=32
        AND step_up_issued_at IS NOT NULL
        AND step_up_expires_at>step_up_issued_at
        AND step_up_expires_at<=step_up_issued_at+INTERVAL '5 minutes'
        AND (step_up_used_at IS NULL OR step_up_used_at>=step_up_issued_at)
    )
);
