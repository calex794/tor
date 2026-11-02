-- C-17: preferences for a FUTURE external notification channel only.
-- The on-site release feed, follow intent, read markers and purchases are independent.
-- No worker or transport is enabled by creating this row.
CREATE TABLE reader_notification_delivery_preferences (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    delivery_mode TEXT NOT NULL DEFAULT 'disabled'
        CHECK (delivery_mode IN ('disabled', 'immediate', 'daily_digest')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE reader_notification_delivery_preferences IS
    'Reader-owned future delivery cadence; default disabled, no channel consent or delivery implied.';
