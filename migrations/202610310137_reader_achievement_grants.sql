-- F-12 / S046: achievements for C-validated unique chapter completions.
-- Append only; do not change the nine donor migrations or C completion facts.
-- The event delivery receipt is the existing event_deliveries table.
CREATE TABLE user_achievement_grants (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    achievement_key TEXT NOT NULL CHECK (
        achievement_key IN (
            'reader_chapters_1','reader_chapters_10','reader_chapters_50',
            'reader_chapters_100','reader_chapters_250','reader_chapters_500'
        )
    ),
    catalog_version SMALLINT NOT NULL DEFAULT 1 CHECK (catalog_version = 1),
    unlocked_at TIMESTAMPTZ NOT NULL,
    source_event_id UUID NOT NULL,
    PRIMARY KEY (user_id, achievement_key)
);
CREATE INDEX user_achievement_grants_source_event
    ON user_achievement_grants(source_event_id);

-- One-time migration of C facts created before achievements-v1 existed.
-- Never rewrite required_consumers on historical, already delivered events.
-- The Nth unique canonical chapter is the actual time/evidence of its badge.
WITH ordered AS (
    SELECT user_id, source_event_id, completed_at,
           row_number() OVER (
               PARTITION BY user_id ORDER BY completed_at,chapter_id
           ) AS ordinal
    FROM reader_chapter_completions
)
INSERT INTO user_achievement_grants
    (user_id,achievement_key,catalog_version,unlocked_at,source_event_id)
SELECT user_id, 'reader_chapters_' || ordinal::text, 1, completed_at, source_event_id
FROM ordered
WHERE ordinal IN (1,10,50,100,250,500)
ON CONFLICT(user_id,achievement_key) DO NOTHING;
