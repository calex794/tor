-- NEwBOO book statistics: displayed free chapter opens are not completions.
-- Anonymous traffic is deliberately not assigned a persistent fingerprint.
-- Per-user sets exist only to count registered readers exactly and are erased
-- by the existing users ON DELETE CASCADE boundary.
CREATE TABLE book_open_readers (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    first_opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (book_id, user_id)
);

CREATE TABLE book_chapter_open_readers (
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    first_opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (chapter_id, user_id)
);
CREATE INDEX book_chapter_open_readers_book
    ON book_chapter_open_readers (book_id, chapter_id);

-- UTC daily counters, not a raw IP/path/session log. Updates are atomic.
-- A repeated visit never increments the unique authenticated counter.
CREATE TABLE book_chapter_open_daily (
    day DATE NOT NULL,
    chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    opens BIGINT NOT NULL DEFAULT 0 CHECK (opens >= 0),
    anonymous_opens BIGINT NOT NULL DEFAULT 0 CHECK (anonymous_opens >= 0),
    authenticated_opens BIGINT NOT NULL DEFAULT 0 CHECK (authenticated_opens >= 0),
    repeat_authenticated_opens BIGINT NOT NULL DEFAULT 0
        CHECK (repeat_authenticated_opens >= 0),
    first_authenticated_opens BIGINT NOT NULL DEFAULT 0
        CHECK (first_authenticated_opens >= 0),
    PRIMARY KEY (day, chapter_id)
);
CREATE INDEX book_chapter_open_daily_book_day
    ON book_chapter_open_daily (book_id, day DESC);

COMMENT ON TABLE book_chapter_open_daily IS
    'Best-effort free chapter HTML opens; not proof of a human reading/completing. No historical backfill.';
