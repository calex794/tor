-- S058: bounded, restart-safe chapter-release fanout into E-08 notifications.
-- The existing reader follow view remains read-only and independent.
-- Production migration rollout is explicitly out of scope.
CREATE TABLE chapter_release_notification_fanout (
    source_event_id UUID PRIMARY KEY REFERENCES outbox_events(id) ON DELETE RESTRICT,
    last_user_id BIGINT NOT NULL DEFAULT 0 CHECK (last_user_id >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
);

COMMENT ON TABLE chapter_release_notification_fanout IS
    'Cursor for bounded, idempotent S058 ChapterPublished to recipient events; never stores chapter text.';
