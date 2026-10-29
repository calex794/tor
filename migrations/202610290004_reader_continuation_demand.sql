-- C/I / Loop 1: explicit intent per completed public frontier; no entitlement/follow.
CREATE TABLE reader_continuation_demand (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY CHECK (id > 0),
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    frontier_chapter_id BIGINT NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, book_id, frontier_chapter_id)
);
CREATE INDEX reader_continuation_demand_frontier_idx
    ON reader_continuation_demand (book_id, frontier_chapter_id);
