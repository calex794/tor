-- MAIL-01g1: recipient-key-only suppression, independent of account lifetime.
CREATE TABLE mail_recipient_suppression (
    recipient_key BYTEA PRIMARY KEY CHECK (octet_length(recipient_key)=32),
    reason TEXT NOT NULL CHECK (reason IN ('hard_bounce','complaint')),
    source_reserved_at TIMESTAMPTZ NOT NULL,
    suppressed_until TIMESTAMPTZ,
    CHECK (
        (reason='complaint' AND suppressed_until IS NULL)
        OR (reason='hard_bounce' AND suppressed_until IS NOT NULL
            AND suppressed_until>source_reserved_at)
    )
);
CREATE INDEX mail_recipient_suppression_expiry
    ON mail_recipient_suppression(suppressed_until)
    WHERE suppressed_until IS NOT NULL;
