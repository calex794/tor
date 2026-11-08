-- NEwBOO S058: book-owned Moscow-time cadence settings and ordered slots.
-- This migration only persists settings; it never arms the worker or releases chapters.
CREATE TABLE book_delayed_publication_settings (
    book_id BIGINT PRIMARY KEY REFERENCES books(id) ON DELETE RESTRICT,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    interval_days INTEGER NOT NULL CHECK (interval_days >= 1),
    anchor_date DATE,
    schedule_revision BIGINT NOT NULL CHECK (schedule_revision > 0),
    next_slot_at TIMESTAMPTZ,
    next_slot_index INTEGER CHECK (next_slot_index BETWEEN 0 AND 99),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_by BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    CONSTRAINT book_schedule_cursor_consistency CHECK (
        (NOT enabled AND anchor_date IS NULL AND next_slot_at IS NULL
                     AND next_slot_index IS NULL)
        OR (enabled AND anchor_date IS NOT NULL AND next_slot_at IS NOT NULL
                    AND next_slot_index IS NOT NULL)
    )
);
CREATE TABLE book_delayed_publication_slots (
    book_id BIGINT NOT NULL REFERENCES book_delayed_publication_settings(book_id)
        ON DELETE CASCADE,
    slot_index INTEGER NOT NULL CHECK (slot_index BETWEEN 0 AND 99),
    minute_of_day INTEGER NOT NULL CHECK (minute_of_day BETWEEN 0 AND 1439),
    PRIMARY KEY (book_id,slot_index)
);
CREATE INDEX book_delayed_publication_due_idx
    ON book_delayed_publication_settings (next_slot_at,book_id)
    WHERE enabled;
