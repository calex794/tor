-- G-07 / RIL-06: additive, best-effort aggregated telemetry.
-- No identifiers, raw paths, IPs or session data are stored.
-- Durable *after enqueue*: pending batches can be replayed following process
-- restart. Individual HTTP requests are never synchronously written to PG.
CREATE TABLE ril_flush_batches (
    batch_id TEXT PRIMARY KEY CHECK (char_length(batch_id) = 36),
    bucket TIMESTAMPTZ NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
      CHECK (status IN ('pending','running','done','dead')),
    lease_epoch BIGINT NOT NULL DEFAULT 0 CHECK (lease_epoch >= 0),
    leased_until TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 16),
    run_after TIMESTAMPTZ NOT NULL DEFAULT now(),
    enqueued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    CHECK (date_trunc('hour', bucket) = bucket)
);
CREATE INDEX ril_flush_claim_idx ON ril_flush_batches(run_after, enqueued_at)
    WHERE status IN ('pending','running');

CREATE TABLE ril_flush_routes (
    batch_id TEXT NOT NULL REFERENCES ril_flush_batches(batch_id) ON DELETE CASCADE,
    route_code SMALLINT NOT NULL CHECK (route_code BETWEEN 0 AND 14),
    status_group SMALLINT NOT NULL CHECK (status_group BETWEEN 0 AND 5),
    latency_group SMALLINT NOT NULL CHECK (latency_group BETWEEN 0 AND 3),
    request_count BIGINT NOT NULL CHECK (request_count BETWEEN 1 AND 1000000000),
    PRIMARY KEY (batch_id,route_code,status_group,latency_group)
);
CREATE TABLE ril_flush_transitions (
    batch_id TEXT NOT NULL REFERENCES ril_flush_batches(batch_id) ON DELETE CASCADE,
    from_code SMALLINT NOT NULL CHECK (from_code BETWEEN 0 AND 14),
    to_code SMALLINT NOT NULL CHECK (to_code BETWEEN 0 AND 14),
    gap_group SMALLINT NOT NULL CHECK (gap_group BETWEEN 0 AND 3),
    transition_count BIGINT NOT NULL CHECK (transition_count BETWEEN 1 AND 1000000000),
    PRIMARY KEY (batch_id,from_code,to_code,gap_group)
);

CREATE TABLE ril_route_hourly (
    bucket TIMESTAMPTZ NOT NULL,
    route_code SMALLINT NOT NULL CHECK (route_code BETWEEN 0 AND 14),
    status_group SMALLINT NOT NULL CHECK (status_group BETWEEN 0 AND 5),
    latency_group SMALLINT NOT NULL CHECK (latency_group BETWEEN 0 AND 3),
    request_count BIGINT NOT NULL CHECK (request_count >= 0),
    PRIMARY KEY (bucket,route_code,status_group,latency_group)
);
CREATE TABLE ril_transition_hourly (
    bucket TIMESTAMPTZ NOT NULL,
    from_code SMALLINT NOT NULL CHECK (from_code BETWEEN 0 AND 14),
    to_code SMALLINT NOT NULL CHECK (to_code BETWEEN 0 AND 14),
    gap_group SMALLINT NOT NULL CHECK (gap_group BETWEEN 0 AND 3),
    transition_count BIGINT NOT NULL CHECK (transition_count >= 0),
    PRIMARY KEY (bucket,from_code,to_code,gap_group)
);
CREATE TABLE ril_daily_rollup (
    day DATE NOT NULL,
    route_code SMALLINT NOT NULL CHECK (route_code BETWEEN 0 AND 14),
    requests BIGINT NOT NULL CHECK (requests >= 0),
    http_errors BIGINT NOT NULL CHECK (http_errors >= 0),
    slow_requests BIGINT NOT NULL CHECK (slow_requests >= 0),
    PRIMARY KEY (day,route_code)
);
-- No immediate DML from the public HTTP request path.
-- RIL's background writer shares the bounded tor_app connection pool.
