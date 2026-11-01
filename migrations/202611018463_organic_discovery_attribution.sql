-- Loop 02 / F: source-verified organic catalogue and search discovery.
-- Raw terms, email, IP, session tokens, and reader chapter text must never enter
-- this schema. Reuse REC-13's consent subject and canonical C read events.
CREATE TABLE organic_discovery_touches (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    session_digest bytea NOT NULL CHECK(octet_length(session_digest)=32),
    ticket_digest bytea NOT NULL UNIQUE CHECK(octet_length(ticket_digest)=32),
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    source text NOT NULL CHECK(source IN ('search','catalog')),
    position smallint NOT NULL CHECK(position BETWEEN 1 AND 20),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL DEFAULT now()+interval '15 minutes',
    clicked_at timestamptz,
    book_opened_at timestamptz,
    started_at timestamptz,
    completed_at timestamptz,
    cohort_day date NOT NULL DEFAULT (now() AT TIME ZONE 'UTC')::date,
    CHECK(expires_at>created_at),
    CHECK(clicked_at IS NULL OR clicked_at>=created_at)
);
CREATE INDEX organic_touches_subject_book_click ON organic_discovery_touches(subject_id,book_id,clicked_at DESC) WHERE clicked_at IS NOT NULL;
CREATE INDEX organic_touches_budget ON organic_discovery_touches(subject_id,created_at);
CREATE INDEX organic_touches_cohort ON organic_discovery_touches(cohort_day,book_id);
CREATE INDEX organic_touches_retention ON organic_discovery_touches(created_at);

-- One authoritative first event may be credited to only one channel.
-- REC-13's existing conversions remain their own table; the F consumer
-- chooses one channel for an event in the same locked outbox transaction.
CREATE TABLE organic_discovery_first_conversions (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK(kind IN ('chapter_started','chapter_finished')),
    touch_id uuid NOT NULL REFERENCES organic_discovery_touches(id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL UNIQUE,
    occurred_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,book_id,kind)
);
-- C-verified first reads by a consented reader with no eligible REC or
-- organic navigation. This is NOT a third traffic source, and no anonymous
-- reader or raw browsing activity is persisted. 30d raw/180d aggregate.
CREATE TABLE organic_discovery_unattributed_reads (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK(kind IN ('chapter_started','chapter_finished')),
    source_event_id uuid NOT NULL UNIQUE,
    occurred_at timestamptz NOT NULL,
    bucket_date date NOT NULL,
    PRIMARY KEY(subject_id,book_id,kind)
);
CREATE INDEX organic_unattributed_retention ON organic_discovery_unattributed_reads(occurred_at);
-- Daily materialization needs a bounded index instead of scanning 30d of C facts.
CREATE INDEX organic_unattributed_quality ON organic_discovery_unattributed_reads(bucket_date,book_id);
CREATE TABLE organic_discovery_dirty (
    bucket_date date NOT NULL,
    book_id bigint NOT NULL,
    PRIMARY KEY(bucket_date,book_id)
);
CREATE TABLE organic_discovery_quality_daily (
    bucket_date date NOT NULL,
    book_id bigint NOT NULL,
    source text NOT NULL CHECK(source IN ('search','catalog','unattributed')),
    issued bigint NOT NULL CHECK(issued>=0),
    clicks bigint NOT NULL CHECK(clicks>=0),
    book_opens bigint NOT NULL CHECK(book_opens>=0),
    starts bigint NOT NULL CHECK(starts>=0),
    completions bigint NOT NULL CHECK(completions>=0),
    refreshed_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(bucket_date,book_id,source)
);
-- Aggregates do not reveal a user, raw query, or unhashed ticket.
