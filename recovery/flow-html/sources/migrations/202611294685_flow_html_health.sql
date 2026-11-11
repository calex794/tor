-- Anonymous HTML collector health only. Epochs identify workers, never readers.
CREATE TABLE ril_flow_health_epochs (
    epoch bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_ms bigint NOT NULL CHECK (started_ms > 0),
    confirmed_ms bigint NOT NULL,
    lease_until_ms bigint NOT NULL,
    generation bigint NOT NULL DEFAULT 0,
    pending_ms bigint,
    retired boolean NOT NULL DEFAULT false,
    dropped bigint NOT NULL DEFAULT 0 CHECK (dropped >= 0),
    failed bigint NOT NULL DEFAULT 0 CHECK (failed >= 0),
    CHECK (confirmed_ms >= started_ms),
    CHECK (lease_until_ms >= confirmed_ms),
    CHECK (pending_ms IS NULL OR pending_ms >= confirmed_ms)
);
CREATE INDEX ril_flow_health_epoch_range ON ril_flow_health_epochs
    USING gist (int8range(started_ms, lease_until_ms, '[)'));
CREATE INDEX ril_flow_health_epoch_expiry ON ril_flow_health_epochs (lease_until_ms);
CREATE TABLE ril_flow_health_hourly (
    epoch bigint NOT NULL REFERENCES ril_flow_health_epochs(epoch),
    bucket_ms bigint NOT NULL CHECK (bucket_ms % 3600000 = 0),
    had_loss boolean NOT NULL DEFAULT false,
    had_error boolean NOT NULL DEFAULT false,
    PRIMARY KEY (epoch, bucket_ms),
    CHECK (had_loss OR had_error)
);
CREATE INDEX ril_flow_health_hour ON ril_flow_health_hourly (bucket_ms);
