-- MAIL-RESET-01a: add a guarded transactional purpose without editing old migrations.
ALTER TABLE mail_outbox DROP CONSTRAINT mail_outbox_purpose_check;
ALTER TABLE mail_outbox ADD CONSTRAINT mail_outbox_purpose_check
    CHECK (purpose IN ('registration_confirmation', 'password_reset', 'support_reply', 'project_news'));

ALTER TABLE mail_send_reservations DROP CONSTRAINT mail_send_reservations_purpose_check;
ALTER TABLE mail_send_reservations ADD CONSTRAINT mail_send_reservations_purpose_check
    CHECK (purpose IN ('registration_confirmation', 'password_reset', 'support_reply', 'project_news'));
ALTER TABLE mail_send_reservations DROP CONSTRAINT mail_send_reservations_check;
-- Preserve the existing column; reset rows use the original reset request IP digest.
ALTER TABLE mail_send_reservations ADD CONSTRAINT mail_send_reservations_check
    CHECK (purpose IN ('registration_confirmation', 'password_reset') OR registration_ip_key IS NULL);
