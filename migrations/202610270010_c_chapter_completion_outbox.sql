-- C/F EVT-03/04: a completion is a server-validated C fact, never a
-- bookmark, chapter GET, S3 download, or client-supplied "trusted" flag.
-- The historical migrations are immutable; this is an additive I-owned schema.
-- One shared durable outbox is used by every domain, not a second C queue.
CREATE TABLE outbox_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type TEXT NOT NULL,
    schema_version SMALLINT NOT NULL CHECK (schema_version > 0),
    aggregate_kind TEXT NOT NULL,
    aggregate_id BIGINT NOT NULL,
    dedupe_key TEXT NOT NULL UNIQUE,
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    required_consumers TEXT[] NOT NULL CHECK (cardinality(required_consumers) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    lease_epoch BIGINT NOT NULL DEFAULT 0,
    delivered_at TIMESTAMPTZ,
    dead_at TIMESTAMPTZ
);
CREATE INDEX outbox_due_idx ON outbox_events (next_attempt_at, created_at, id)
    WHERE delivered_at IS NULL AND dead_at IS NULL;

CREATE TABLE event_deliveries (
    consumer TEXT NOT NULL,
    event_id UUID NOT NULL REFERENCES outbox_events(id) ON DELETE RESTRICT,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id)
);

-- The first completed edition is retained even if a later edition replaces it.
-- A second device, repeated PUT, or subsequent V2 cannot create another fact.
CREATE TABLE reader_chapter_completions (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    first_version_id BIGINT NOT NULL,
    completed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    source_event_id UUID NOT NULL UNIQUE,
    PRIMARY KEY (user_id,book_id,chapter_id),
    FOREIGN KEY (chapter_id,first_version_id)
        REFERENCES chapter_versions(chapter_id,id) ON DELETE NO ACTION DEFERRABLE INITIALLY DEFERRED
);
CREATE INDEX reader_chapter_completions_book
    ON reader_chapter_completions(user_id,book_id,completed_at DESC);
