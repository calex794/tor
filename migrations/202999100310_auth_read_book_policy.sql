-- AUTH-READ-01b: additive, inactive book guest-reading policy.
-- The old reader still serves FREE text; prevent restriction at the DB
-- boundary until an independently accepted migration removes this fence.
ALTER TABLE books
    ADD COLUMN guest_read_mode TEXT NOT NULL DEFAULT 'open'
        CHECK (guest_read_mode IN ('open','login_preview')),
    ADD COLUMN guest_open_through_chapter_no INTEGER NOT NULL DEFAULT 0
        CHECK (guest_open_through_chapter_no >= 0),
    ADD COLUMN guest_preview_percent SMALLINT NOT NULL DEFAULT 20
        CHECK (guest_preview_percent IN (10,20,30,50)),
    ADD COLUMN guest_policy_revision BIGINT NOT NULL DEFAULT 0
        CHECK (guest_policy_revision >= 0),
    ADD CONSTRAINT books_guest_read_rollout_closed CHECK (guest_read_mode='open');

-- Each successful field-scoped CAS produces one immutable revision receipt.
-- No chapter text, session digest, prices or financial data in these rows.
CREATE TABLE book_guest_read_policy_events (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    revision BIGINT NOT NULL CHECK (revision > 0),
    actor_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    old_mode TEXT NOT NULL CHECK (old_mode IN ('open','login_preview')),
    new_mode TEXT NOT NULL CHECK (new_mode IN ('open','login_preview')),
    old_open_through INTEGER NOT NULL CHECK (old_open_through >= 0),
    new_open_through INTEGER NOT NULL CHECK (new_open_through >= 0),
    old_preview_percent SMALLINT NOT NULL CHECK (old_preview_percent IN (10,20,30,50)),
    new_preview_percent SMALLINT NOT NULL CHECK (new_preview_percent IN (10,20,30,50)),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (book_id,revision),
    CHECK ((old_mode,old_open_through,old_preview_percent)
           IS DISTINCT FROM (new_mode,new_open_through,new_preview_percent))
);

CREATE FUNCTION auth_read_policy_event_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'guest reading policy receipt is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER auth_read_policy_event_immutable
    BEFORE UPDATE OR DELETE ON book_guest_read_policy_events
    FOR EACH ROW EXECUTE FUNCTION auth_read_policy_event_immutable();
