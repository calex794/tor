-- MAIL-RESET-03c2: private test purpose, never exposed in product MailPurpose.
-- The same quota ledger, feedback IDs and billing month are used.
ALTER TABLE mail_outbox DROP CONSTRAINT mail_outbox_purpose_check;
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_purpose_check
    CHECK (purpose IN ('registration_confirmation','password_reset','support_reply','project_news','operator_test'));
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_operator_test_one_claim
    CHECK (purpose <> 'operator_test' OR (
        generation=1 AND status IN ('leased','accepted','failed','uncertain','cancelled')
        AND next_attempt_at IS NULL AND defer_reason IS NULL
    ));

ALTER TABLE mail_send_reservations DROP CONSTRAINT mail_send_reservations_purpose_check;
ALTER TABLE mail_send_reservations ADD CONSTRAINT mail_send_reservations_purpose_check
    CHECK (purpose IN ('registration_confirmation','password_reset','support_reply','project_news','operator_test'));
ALTER TABLE mail_send_reservations ADD CONSTRAINT mail_operator_test_reservation_binding
    CHECK (purpose <> 'operator_test' OR (generation=1 AND launch_intent_digest IS NOT NULL));
CREATE UNIQUE INDEX mail_operator_test_activation_once
    ON mail_send_reservations(launch_intent_digest) WHERE purpose='operator_test';
