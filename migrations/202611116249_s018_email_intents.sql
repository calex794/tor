-- E/S018: durable intentions only; NO transport or automatic worker.
-- Each already-authorized in-app chapter notification may be staged once for
-- verified, explicit email consent and immediate timing. Nothing is sent.
CREATE TABLE reader_email_notification_intents (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    notification_id bigint NOT NULL UNIQUE
        REFERENCES user_notifications(id) ON DELETE CASCADE,
    recipient_user_id bigint NOT NULL
        REFERENCES users(id) ON DELETE CASCADE,
    delivery_mode text NOT NULL DEFAULT 'immediate'
        CHECK(delivery_mode='immediate'),
    state text NOT NULL DEFAULT 'queued'
        CHECK(state IN ('queued','leased','delivered','suppressed')),
    due_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    queued_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    lease_owner uuid,
    lease_epoch bigint NOT NULL DEFAULT 0 CHECK(lease_epoch>=0),
    leased_until timestamptz,
    attempts smallint NOT NULL DEFAULT 0 CHECK(attempts BETWEEN 0 AND 8),
    delivered_at timestamptz,
    CONSTRAINT s018_email_intent_lease_pair
        CHECK((lease_owner IS NULL)=(leased_until IS NULL)),
    CONSTRAINT s018_email_intent_delivered_state
        CHECK(delivered_at IS NULL OR state='delivered')
);
CREATE INDEX reader_email_notification_intents_due
    ON reader_email_notification_intents(due_at,id)
    WHERE state='queued';
COMMENT ON TABLE reader_email_notification_intents IS
    'Reference IDs only. No recipients, chapter title, body, tokens, or content; external delivery stays disabled.';
