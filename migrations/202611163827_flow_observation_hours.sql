-- FLOW-02/04: server observation-hour public HTML counts, no visitor identity.
-- Uses existing RIL batch UUID/claim/fence/transaction. No second queue or outbox.
-- Do not run on production as part of development.
ALTER TABLE ril_flush_batches ADD COLUMN flow_observed_discarded BIGINT NOT NULL DEFAULT 0
    CHECK (flow_observed_discarded >= 0);

CREATE TABLE ril_flush_flow_observed (
    batch_id TEXT NOT NULL REFERENCES ril_flush_batches(batch_id) ON DELETE CASCADE,
    bucket TIMESTAMPTZ NOT NULL CHECK (
        bucket = date_trunc('hour', bucket AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'),
    surface_code SMALLINT NOT NULL CHECK (surface_code BETWEEN 0 AND 3),
    acquisition_hint SMALLINT NOT NULL CHECK (acquisition_hint BETWEEN 0 AND 3),
    html_requests BIGINT NOT NULL CHECK (html_requests BETWEEN 1 AND 1000000000),
    PRIMARY KEY (batch_id, bucket, surface_code, acquisition_hint)
);

CREATE TABLE ril_flow_observed_hourly (
    bucket TIMESTAMPTZ NOT NULL CHECK (
        bucket = date_trunc('hour', bucket AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'),
    surface_code SMALLINT NOT NULL CHECK (surface_code BETWEEN 0 AND 3),
    acquisition_hint SMALLINT NOT NULL CHECK (acquisition_hint BETWEEN 0 AND 3),
    html_requests BIGINT NOT NULL CHECK (html_requests >= 0),
    PRIMARY KEY (bucket, surface_code, acquisition_hint)
);

-- Thin projection receipts outlive the 180-day anonymous hourly retention.
-- They intentionally have no FK: legacy RIL tombstones expire after 90 days.
-- A very late replay must not reapply an observed-hour projection still retained.
CREATE TABLE ril_flow_observed_receipts (
    batch_id TEXT PRIMARY KEY CHECK (length(batch_id) = 36),
    materialized_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    discarded_events BIGINT NOT NULL CHECK (discarded_events >= 0)
);
CREATE INDEX ril_flow_observed_receipts_expiry ON ril_flow_observed_receipts(materialized_at);
-- No backfill from ril_flow_hourly: persistence time cannot recover event time.

-- A rolling old worker cannot silently mark a new observation batch done.
-- It rolls back instead; an upgraded worker can claim and apply the same batch.
CREATE FUNCTION ril_flow_observed_guard_done() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog, public AS $$
BEGIN
    IF (NEW.flow_observed_discarded > 0 OR EXISTS (
        SELECT 1 FROM public.ril_flush_flow_observed WHERE batch_id=NEW.batch_id))
       AND NOT EXISTS (
        SELECT 1 FROM public.ril_flow_observed_receipts WHERE batch_id=NEW.batch_id)
    THEN
        RAISE EXCEPTION 'RIL observed-hour projection must commit with batch completion'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER ril_flow_observed_requires_receipt
BEFORE UPDATE OF status ON ril_flush_batches
FOR EACH ROW WHEN (NEW.status='done') EXECUTE FUNCTION ril_flow_observed_guard_done();
