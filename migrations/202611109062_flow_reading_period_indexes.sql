-- C/I FLOW: the administrator reads bounded time intervals over canonical facts.
-- Existing indexes lead with a reader/book and cannot bound a site-wide time scan.
-- No new event store, identity, retention policy or mutable business fact.
CREATE INDEX reader_chapter_starts_audience_period
    ON reader_chapter_starts (started_at);

CREATE INDEX reader_chapter_completions_audience_period
    ON reader_chapter_completions (completed_at);
