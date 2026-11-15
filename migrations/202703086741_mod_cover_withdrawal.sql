-- MOD-COVER: keep the slot revision monotonic when a site moderator
-- withdraws a published image. A NULL/deleted slot would reset expected
-- revisions to zero and allow stale proposals to be approved.
ALTER TABLE book_cover_slots
    ADD COLUMN is_withdrawn BOOLEAN NOT NULL DEFAULT FALSE;

-- Removal of the CURRENT approved public slot is separate from rejecting an
-- unapproved candidate. Preserve the old job and review for audit; never
-- delete the immutable private source or paid objects from this command.
CREATE TABLE book_cover_slot_withdrawals (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    job_id BIGINT NOT NULL REFERENCES cover_variant_jobs(id) ON DELETE RESTRICT,
    previous_revision BIGINT NOT NULL CHECK (previous_revision > 0),
    new_revision BIGINT NOT NULL CHECK (new_revision = previous_revision + 1),
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    reason TEXT NOT NULL CHECK (char_length(btrim(reason)) BETWEEN 1 AND 2000),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (book_id, previous_revision)
);
CREATE INDEX book_cover_slot_withdrawals_book_history
    ON book_cover_slot_withdrawals(book_id,id DESC);

-- The old approval history remains immutable; withdrawals are also immutable.
CREATE TRIGGER cover_slot_withdrawals_append_only
    BEFORE UPDATE OR DELETE ON book_cover_slot_withdrawals
    FOR EACH ROW EXECUTE FUNCTION refuse_cover_decision_change();
