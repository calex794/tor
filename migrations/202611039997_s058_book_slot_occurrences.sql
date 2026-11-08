-- S058: immutable book-slot identity and token-fenced durable claims.
-- Never releases a chapter or enables a background worker.
CREATE TABLE book_delayed_publication_occurrences (
    book_id BIGINT NOT NULL REFERENCES book_delayed_publication_settings(book_id)
        ON DELETE CASCADE,
    schedule_revision BIGINT NOT NULL CHECK (schedule_revision > 0),
    local_date DATE NOT NULL,
    slot_index INTEGER NOT NULL CHECK (slot_index BETWEEN 0 AND 99),
    target_at TIMESTAMPTZ NOT NULL,
    chapter_id BIGINT REFERENCES chapters(id) ON DELETE RESTRICT,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','processing','completed','empty','blocked','cancelled')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error_code TEXT CHECK (last_error_code IS NULL OR char_length(last_error_code) <= 128),
    processing_token TEXT,
    processing_lease_expires_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    PRIMARY KEY(book_id,schedule_revision,local_date,slot_index),
    CONSTRAINT book_slot_lease_guard CHECK (
        (status='processing') =
        (processing_token IS NOT NULL AND processing_lease_expires_at IS NOT NULL)
    )
);
CREATE INDEX book_slot_occurrence_by_book
    ON book_delayed_publication_occurrences(book_id,target_at,slot_index);
CREATE INDEX book_slot_occurrence_lease
    ON book_delayed_publication_occurrences(status,processing_lease_expires_at)
    WHERE status IN ('pending','processing');
