-- G-09 operator action audit. No S3 URLs, object keys, secrets or content.
CREATE TABLE g09_operator_requests (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    actor_user_id BIGINT NOT NULL REFERENCES users(id),
    direction TEXT NOT NULL CHECK (direction IN ('s3_to_pg','pg_to_s3')),
    scope TEXT NOT NULL CHECK (scope IN ('chapter','chapter_asset')),
    action TEXT NOT NULL CHECK (action IN ('step','next_cycle','resume_exhausted')),
    requested_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (action <> 'resume_exhausted' OR direction = 'pg_to_s3')
);
CREATE INDEX g09_operator_requests_recent ON g09_operator_requests(requested_at DESC);
