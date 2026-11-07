-- MOD-OPS-ALERT-01 / #661, stage 1: durable internal observations only.
-- There is no scheduler, delivery mechanism, public endpoint, or accepted SLA.
-- Do not roll out this migration on persistent databases without approval.
CREATE TABLE moderation_overdue_signals (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_kind TEXT NOT NULL
      CHECK (request_kind IN ('registration','cover','deletion')),
    source_id BIGINT NOT NULL CHECK (source_id > 0),
    source_revision BIGINT NOT NULL CHECK (source_revision >= 0),
    threshold_hours SMALLINT NOT NULL CHECK (threshold_hours IN (24,72)),
    signal_state TEXT NOT NULL DEFAULT 'open'
      CHECK (signal_state IN ('open','resolved')),
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    resolved_at TIMESTAMPTZ,
    CHECK ((signal_state='resolved') = (resolved_at IS NOT NULL)),
    UNIQUE (request_kind,source_id,source_revision,threshold_hours)
);
CREATE INDEX moderation_overdue_signals_unresolved
    ON moderation_overdue_signals(first_seen_at,id)
    WHERE signal_state='open';

-- Only an operational scan can mark an observation resolved.
-- A future delivery worker must re-check the live request inside its own
-- transaction; a ledger record must never authorize sending by itself.
-- No Book title, user ID, reviewer identity, email or S3 staging key is stored.
