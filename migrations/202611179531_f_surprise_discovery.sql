-- F-SURPRISE-01. Non-monetary editorial discoveries, disabled until operator opt-in.
-- Additive schema only. No wallet, payout, Chapter Ads or private book copies.
CREATE TABLE surprise_runtime_policy (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    enabled BOOLEAN NOT NULL DEFAULT false,
    probability_per_mille SMALLINT NOT NULL DEFAULT 350
        CHECK (probability_per_mille BETWEEN 0 AND 1000),
    revision BIGINT NOT NULL DEFAULT 0 CHECK (revision >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO surprise_runtime_policy(singleton) VALUES(true);

CREATE TABLE surprise_content (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('joke','quote','book','fact')),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    body TEXT NOT NULL CHECK (char_length(body) BETWEEN 1 AND 600),
    book_id BIGINT REFERENCES books(id) ON DELETE CASCADE,
    editorial BOOLEAN NOT NULL DEFAULT false,
    state TEXT NOT NULL DEFAULT 'draft'
        CHECK (state IN ('draft','approved','paused','archived')),
    starts_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ends_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at IS NULL OR ends_at > starts_at),
    CHECK (kind <> 'book' OR book_id IS NOT NULL)
);
CREATE INDEX surprise_approved_pool
    ON surprise_content(starts_at,id) WHERE state='approved';

CREATE TABLE surprise_user_preferences (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    disabled BOOLEAN NOT NULL DEFAULT false,
    last_shown_at TIMESTAMPTZ,
    backoff_until TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One decision for a reader per UTC day, including a negative decision. Reload,
-- another device and concurrent HTTP requests must not be extra chances.
CREATE TABLE surprise_daily_decisions (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    utc_day BIGINT NOT NULL CHECK (utc_day > 18000),
    state TEXT NOT NULL
        CHECK (state IN ('not_selected','planned','claimed','opened','dismissed')),
    active_after_seconds INTEGER CHECK (active_after_seconds BETWEEN 120 AND 1200),
    anchor_ticket BIGINT,
    content_ticket BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_at TIMESTAMPTZ,
    opened_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, utc_day),
    CONSTRAINT surprise_state_consistency CHECK (
        (state='not_selected' AND active_after_seconds IS NULL
             AND anchor_ticket IS NULL AND content_ticket IS NULL)
        OR (state<>'not_selected' AND active_after_seconds IS NOT NULL
             AND anchor_ticket IS NOT NULL AND content_ticket IS NOT NULL)
    )
);
CREATE INDEX surprise_decisions_retention
    ON surprise_daily_decisions(created_at);

COMMENT ON TABLE surprise_daily_decisions IS
    'F-owned bounded nonfinancial surprise decisions; no tracking of chapters, clicks or raw browsing.';
