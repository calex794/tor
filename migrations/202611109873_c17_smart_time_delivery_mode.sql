-- C-17 / E S018: keep opt-in delivery cadence compatible with NEwBOO.
-- The existing default stays disabled; no scheduler, provider, or messages.
-- Widen the existing column-mode CHECK without weakening account scoping.
ALTER TABLE reader_notification_delivery_preferences
    DROP CONSTRAINT reader_notification_delivery_preferences_delivery_mode_check,
    ADD CONSTRAINT c17_notification_delivery_mode_check
        CHECK (delivery_mode IN ('disabled','immediate','smart_time','daily_digest'));
