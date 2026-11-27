-- Bind real dispatch reservations to one exact activation receipt.
-- Legacy offline reservations retain NULL; no history is rewritten.
ALTER TABLE mail_send_reservations ADD COLUMN activation_digest BYTEA
    CHECK (activation_digest IS NULL OR octet_length(activation_digest)=32);
