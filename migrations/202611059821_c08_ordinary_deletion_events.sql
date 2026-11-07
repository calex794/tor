-- BR-060: owner step-up ordinary deletion audit.
-- Audit facts do not reference hard-deletable rows by foreign key, so later
-- security/legal erasure can follow its own policy without rewriting history.
CREATE TABLE book_ordinary_deletion_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL CHECK (book_id>0),
    actor_user_id BIGINT NOT NULL CHECK (actor_user_id>0),
    action TEXT NOT NULL CHECK (action IN ('start','complete')),
    previous_status TEXT NOT NULL,
    next_status TEXT NOT NULL,
    previous_revision BIGINT NOT NULL CHECK (previous_revision>0),
    next_revision BIGINT NOT NULL CHECK (next_revision=previous_revision+1),
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (
       (action='start' AND next_status='pending_deletion')
       OR (action='complete' AND previous_status='pending_deletion'
           AND next_status='ordinary_deleted')
    )
);
CREATE INDEX book_ordinary_deletion_events_book_recent
    ON book_ordinary_deletion_events(book_id,id DESC);

CREATE FUNCTION book_ordinary_deletion_event_immutable()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'ordinary deletion events are append-only' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER book_ordinary_deletion_events_immutable
 BEFORE UPDATE OR DELETE ON book_ordinary_deletion_events
 FOR EACH ROW EXECUTE FUNCTION book_ordinary_deletion_event_immutable();
