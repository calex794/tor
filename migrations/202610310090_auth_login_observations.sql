-- A-09 / AB-02: private outcome facts for site, source and target analytics.
-- No raw email, IP, password, session token or User-Agent is stored here.
-- HMAC keys are app-side; even unknown target addresses stay pseudonymous.
-- New table only: never rewrite previously applied SQLx migrations.
CREATE TABLE auth_login_observations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    outcome TEXT NOT NULL CHECK (outcome IN (
        'success', 'bad_password', 'unknown_account', 'rate_limited',
        'invalid_form', 'unavailable', 'worker_busy'
    )),
    target_key BYTEA CHECK (target_key IS NULL OR octet_length(target_key)=32),
    user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    source_key BYTEA CHECK (source_key IS NULL OR octet_length(source_key)=32),
    subnet_key BYTEA CHECK (subnet_key IS NULL OR octet_length(subnet_key)=32),
    agent_family TEXT NOT NULL CHECK (agent_family IN (
        'edge', 'chrome', 'firefox', 'safari', 'other', 'unknown'
    ))
);
CREATE INDEX auth_login_observations_time
    ON auth_login_observations (observed_at, id);
CREATE INDEX auth_login_observations_by_target
    ON auth_login_observations (target_key, observed_at DESC)
    WHERE target_key IS NOT NULL;
CREATE INDEX auth_login_observations_by_source
    ON auth_login_observations (source_key, observed_at DESC)
    WHERE source_key IS NOT NULL;

-- Read-only operator views. The public HTTP router must NOT expose these.
-- 7-day detail lifetime is enforced by PgAccounts::prune_security_data.
CREATE VIEW auth_observation_hourly AS
SELECT date_trunc('hour', observed_at) AS hour_utc,
       count(*) AS attempts,
       count(*) FILTER (WHERE outcome='success') AS successes,
       count(*) FILTER (WHERE outcome IN ('bad_password','unknown_account')) AS rejected,
       count(*) FILTER (WHERE outcome='unknown_account') AS unknown_accounts,
       count(*) FILTER (WHERE outcome='rate_limited') AS rate_limited,
       count(DISTINCT target_key) AS distinct_targets,
       count(DISTINCT source_key) AS distinct_sources
FROM auth_login_observations
WHERE observed_at >= now()-INTERVAL '7 days'
GROUP BY date_trunc('hour', observed_at);

CREATE VIEW auth_observation_sources_24h AS
SELECT source_key, count(*) AS attempts,
       count(DISTINCT target_key) AS distinct_targets,
       count(*) FILTER (WHERE outcome='success') AS successes,
       count(*) FILTER (WHERE outcome='unknown_account') AS unknown_accounts,
       count(*) FILTER (WHERE outcome='bad_password') AS bad_passwords
FROM auth_login_observations
WHERE observed_at >= now()-INTERVAL '24 hours' AND source_key IS NOT NULL
GROUP BY source_key;

CREATE VIEW auth_observation_targets_24h AS
SELECT target_key, max(user_id) AS known_user_id,
       count(*) AS attempts,
       count(DISTINCT source_key) AS distinct_sources,
       count(*) FILTER (WHERE outcome='success') AS successes,
       count(*) FILTER (WHERE outcome='unknown_account') AS unknown_accounts,
       count(*) FILTER (WHERE outcome='bad_password') AS bad_passwords
FROM auth_login_observations
WHERE observed_at >= now()-INTERVAL '24 hours' AND target_key IS NOT NULL
GROUP BY target_key;
