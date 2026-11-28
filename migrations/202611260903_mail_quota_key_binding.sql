-- MAIL-01f2a: pin the versioned quota HMAC key, not the secret itself.
-- Unbound historical reservations require explicit recovery, not a reset.
ALTER TABLE mail_send_budget_guard
    ADD COLUMN key_binding BYTEA CHECK (octet_length(key_binding) = 32);
