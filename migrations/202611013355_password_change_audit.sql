-- A-07: successful password change is a new audited session.
-- Existing rows and SQLx historical migrations are never rewritten.
-- Verified IP is pruned after 30 days by existing security retention.
ALTER TABLE account_login_events
    DROP CONSTRAINT account_login_events_event_kind_check;
ALTER TABLE account_login_events
    ADD CONSTRAINT account_login_events_event_kind_check
    CHECK (event_kind IN ('registration', 'password_login', 'password_change'));
