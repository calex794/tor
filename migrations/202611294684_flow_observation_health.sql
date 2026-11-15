-- Health for registered anonymous observation workers. An expired lease is NOT
-- evidence that a process stopped serving requests. Unresolved epochs stay open.
CREATE TABLE flow_observation_epochs (
 epoch bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 registration_key text UNIQUE CHECK(registration_key ~ '^[0-9a-f]{32}$'),
 started_ms bigint NOT NULL CHECK(started_ms>=0),
 confirmed_ms bigint NOT NULL CHECK(confirmed_ms>=started_ms),
 lease_until_ms bigint NOT NULL CHECK(lease_until_ms>=confirmed_ms),
 retired boolean NOT NULL DEFAULT false,
 ended_ms bigint,
 dropped_seen bigint NOT NULL DEFAULT 0 CHECK(dropped_seen>=0),
 failed_seen bigint NOT NULL DEFAULT 0 CHECK(failed_seen>=0),
 pending_cut_ms bigint,
 pending_target bigint,
 CHECK((retired AND ended_ms IS NOT NULL AND ended_ms>=confirmed_ms) OR (NOT retired AND ended_ms IS NULL)),
 CHECK((pending_cut_ms IS NULL AND pending_target IS NULL)
    OR (pending_cut_ms IS NOT NULL AND pending_target IS NOT NULL AND pending_cut_ms>=confirmed_ms AND pending_target>=0))
);
CREATE INDEX flow_observation_epoch_window ON flow_observation_epochs(started_ms,confirmed_ms);
CREATE INDEX flow_observation_epoch_unresolved ON flow_observation_epochs(started_ms) WHERE NOT retired;
CREATE INDEX flow_observation_epoch_retention ON flow_observation_epochs(ended_ms,epoch) WHERE retired;
CREATE TABLE flow_observation_health_hourly (
 epoch bigint NOT NULL REFERENCES flow_observation_epochs(epoch),
 bucket_ms bigint NOT NULL CHECK(bucket_ms>=0 AND bucket_ms%3600000=0),
 lost boolean NOT NULL DEFAULT false,
 failed boolean NOT NULL DEFAULT false,
 uncertain boolean NOT NULL DEFAULT false,
 PRIMARY KEY(epoch,bucket_ms)
);
CREATE INDEX flow_observation_health_window ON flow_observation_health_hourly(bucket_ms,epoch);
