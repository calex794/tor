-- MAIL 4/6: bounded, generation-preserving deferral before any reservation.
-- Identify only the old initial-generation constraint; do not remove the
-- separate lease/finished/status checks or modify a previously applied file.
DO $$
DECLARE old_name text;
BEGIN
    SELECT conname INTO STRICT old_name
    FROM pg_constraint
    WHERE conrelid='mail_outbox'::regclass AND contype='c'
      AND pg_get_constraintdef(oid) LIKE '%generation = 0%'
      AND pg_get_constraintdef(oid) LIKE '%pending%';
    EXECUTE format('ALTER TABLE mail_outbox DROP CONSTRAINT %I', old_name);
END $$;
ALTER TABLE mail_outbox
    ADD CONSTRAINT mail_outbox_claim_generation CHECK (status='pending' OR generation>0),
    ADD COLUMN not_before TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN defer_reason TEXT CHECK (defer_reason IN (
        'source_unavailable','dns_unavailable','provider_unavailable','global_pace',
        'recipient_cooldown','quota_window','budget_paused'
    ));
CREATE INDEX mail_outbox_due ON mail_outbox (not_before,id)
    WHERE status='pending';
