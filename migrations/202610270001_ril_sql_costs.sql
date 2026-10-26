-- RIL-10: optional aggregate SQLx query completion evidence, no SQL, IDs or URLs.
-- The measurement is client-observed SQLx elapsed time, NOT server CPU time.
-- Atomic with existing RIL-06 batch/materialization and its lease epoch.
CREATE TABLE ril_flush_sql_costs (
    batch_id TEXT NOT NULL REFERENCES ril_flush_batches(batch_id) ON DELETE CASCADE,
    route_code SMALLINT NOT NULL CHECK (route_code BETWEEN 0 AND 14),
    observed_requests BIGINT NOT NULL CHECK (observed_requests BETWEEN 1 AND 1000000000),
    observed_queries BIGINT NOT NULL CHECK (observed_queries BETWEEN 0 AND 1000000000),
    sqlx_elapsed_us BIGINT NOT NULL CHECK (sqlx_elapsed_us BETWEEN 0 AND 1000000000),
    PRIMARY KEY (batch_id,route_code)
);
CREATE TABLE ril_sql_cost_hourly (
    bucket TIMESTAMPTZ NOT NULL,
    route_code SMALLINT NOT NULL CHECK (route_code BETWEEN 0 AND 14),
    observed_requests BIGINT NOT NULL CHECK (observed_requests >= 0),
    observed_queries BIGINT NOT NULL CHECK (observed_queries >= 0),
    sqlx_elapsed_us BIGINT NOT NULL CHECK (sqlx_elapsed_us >= 0),
    PRIMARY KEY (bucket,route_code)
);
-- No individual SQL statements, raw paths, user IDs or request IDs are stored.
