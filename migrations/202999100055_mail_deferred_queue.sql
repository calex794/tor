-- MAIL 4b: uncharged pre-submission deferrals; never reset claim generation.
ALTER TABLE mail_outbox DROP CONSTRAINT mail_outbox_status_check;
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_status_check
    CHECK (status IN ('pending','deferred','leased','accepted','failed','uncertain','cancelled'));
ALTER TABLE mail_outbox ADD COLUMN next_attempt_at TIMESTAMPTZ,
    ADD CHECK ((status='deferred')=(next_attempt_at IS NOT NULL));
CREATE INDEX mail_outbox_deferred_due ON mail_outbox(purpose,next_attempt_at,id)
    WHERE status='deferred';
