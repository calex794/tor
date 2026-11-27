-- MAIL 4a: bind a newly charged reservation to one immutable launch receipt.
-- Historical/offline reservations remain NULL, never live dispatch permits.
ALTER TABLE mail_send_reservations
    ADD COLUMN launch_intent_digest BYTEA REFERENCES mail_launch_activation_audit(intent_digest),
    ADD CHECK (launch_intent_digest IS NULL OR octet_length(launch_intent_digest)=32);
