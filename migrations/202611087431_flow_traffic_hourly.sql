-- FLOW-02 (I-owned): only finite anonymous HTML aggregate counters.
-- No IP, cookie, account ID, HMAC subject, raw route, User-Agent or referrer.
-- This is an additive schema, not an instruction to apply migrations on VPS.
-- The existing RIL batch lease/epoch fence makes hourly materialization exact-once.
CREATE TABLE ril_flush_flow (
    batch_id TEXT NOT NULL REFERENCES ril_flush_batches(batch_id) ON DELETE CASCADE,
    surface_code SMALLINT NOT NULL CHECK (surface_code BETWEEN 0 AND 4),
    acquisition_hint SMALLINT NOT NULL CHECK (acquisition_hint BETWEEN 0 AND 3),
    html_requests BIGINT NOT NULL CHECK (html_requests BETWEEN 1 AND 1000000000),
    opted_in_html_requests BIGINT NOT NULL CHECK (
        opted_in_html_requests BETWEEN 0 AND html_requests
    ),
    observed_opted_in_visit_starts BIGINT NOT NULL CHECK (
        observed_opted_in_visit_starts BETWEEN 0 AND opted_in_html_requests
    ),
    PRIMARY KEY (batch_id, surface_code, acquisition_hint)
);

CREATE TABLE ril_flow_hourly (
    bucket TIMESTAMPTZ NOT NULL CHECK (date_trunc('hour', bucket) = bucket),
    surface_code SMALLINT NOT NULL CHECK (surface_code BETWEEN 0 AND 4),
    acquisition_hint SMALLINT NOT NULL CHECK (acquisition_hint BETWEEN 0 AND 3),
    html_requests BIGINT NOT NULL CHECK (html_requests >= 0),
    opted_in_html_requests BIGINT NOT NULL CHECK (
        opted_in_html_requests BETWEEN 0 AND html_requests
    ),
    observed_opted_in_visit_starts BIGINT NOT NULL CHECK (
        observed_opted_in_visit_starts BETWEEN 0 AND opted_in_html_requests
    ),
    PRIMARY KEY (bucket, surface_code, acquisition_hint)
);

-- Row count is bounded at five surfaces x four source hints per hour.
-- Retention follows the RIL background pump (30 days); canonical C/D events
-- and their financial retention are unaffected.
