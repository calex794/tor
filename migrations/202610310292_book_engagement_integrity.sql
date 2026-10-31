-- B: persistent, shared anti-abuse counters for book likes.
-- Ten-second buckets approximate the donor's rolling windows across web instances.
-- Scope hashes avoid raw composite keys in indexes; FKs allow account/book
-- erasure to remove corresponding buckets immediately.
ALTER TABLE book_likes
    ADD COLUMN stats_trusted BOOLEAN NOT NULL DEFAULT TRUE;

CREATE INDEX book_likes_trusted_book_count_idx
    ON book_likes (book_id) WHERE stats_trusted;

CREATE TABLE book_engagement_rate_buckets (
    scope_hash BYTEA NOT NULL CHECK (octet_length(scope_hash) = 32),
    user_id BIGINT REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT REFERENCES books(id) ON DELETE CASCADE,
    bucket_start TIMESTAMPTZ NOT NULL,
    attempts INTEGER NOT NULL CHECK (attempts > 0),
    PRIMARY KEY (scope_hash, bucket_start),
    CHECK ((user_id IS NOT NULL) <> (book_id IS NOT NULL))
);

CREATE INDEX book_engagement_rate_buckets_started_idx
    ON book_engagement_rate_buckets (bucket_start);

-- PostgreSQL does not index FK columns automatically. These indexes keep
-- user/book erasure from scanning every live ten-second rate bucket.
CREATE INDEX book_engagement_rate_buckets_user_idx
    ON book_engagement_rate_buckets (user_id) WHERE user_id IS NOT NULL;

CREATE INDEX book_engagement_rate_buckets_book_idx
    ON book_engagement_rate_buckets (book_id) WHERE book_id IS NOT NULL;

-- Persist only risk observations, not every successful click. No IP, session
-- token, User-Agent, raw URL, or behavior-path history is stored here.
CREATE TABLE book_engagement_integrity_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    action TEXT NOT NULL CHECK (action IN ('book_like', 'book_unlike')),
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Only hard-limit decisions populate this slot. One refusal per
    -- user/book/action/minute is enough to audit automated throttling.
    hard_limit_minute TIMESTAMPTZ,
    subject_action_1m BIGINT NOT NULL CHECK (subject_action_1m >= 0),
    subject_action_1h BIGINT NOT NULL CHECK (subject_action_1h >= 0),
    subject_any_1m BIGINT NOT NULL CHECK (subject_any_1m >= 0),
    target_action_10m BIGINT NOT NULL CHECK (target_action_10m >= 0),
    risk_score SMALLINT NOT NULL CHECK (risk_score BETWEEN 0 AND 100),
    risk_bucket TEXT NOT NULL CHECK
        (risk_bucket IN ('low', 'watch', 'medium', 'high', 'critical')),
    recommended_action TEXT NOT NULL CHECK
        (recommended_action IN ('allow', 'observe', 'throttle')),
    decision TEXT NOT NULL CHECK (decision IN ('allow', 'observe', 'throttle')),
    stats_trusted BOOLEAN NOT NULL,
    reasons TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[]
);

CREATE INDEX book_engagement_integrity_events_observed_idx
    ON book_engagement_integrity_events (observed_at);

CREATE INDEX book_engagement_integrity_events_user_idx
    ON book_engagement_integrity_events (user_id, observed_at DESC);

CREATE INDEX book_engagement_integrity_events_book_idx
    ON book_engagement_integrity_events (book_id, observed_at DESC);

-- The HTTP hard limit is per user, not per book or action. A caller
-- must not bypass audit deduplication by rotating book IDs or like/unlike.
-- Only the first refused request is retained in each UTC minute.
CREATE UNIQUE INDEX book_engagement_integrity_hard_limit_idx
    ON book_engagement_integrity_events (user_id, hard_limit_minute)
    WHERE hard_limit_minute IS NOT NULL;
