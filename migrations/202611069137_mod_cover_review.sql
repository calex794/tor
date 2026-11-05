-- MOD-COVER-01: a processed cover is a PRIVATE candidate, never public.
-- Every first/replacement cover requires a live site-moderator decision.
-- Historical slots remain untouched; future slot INSERT/UPDATE is gated.
CREATE TABLE book_cover_review_requests (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    job_id BIGINT NOT NULL UNIQUE REFERENCES cover_variant_jobs(id) ON DELETE RESTRICT,
    submitted_by_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    expected_slot_revision BIGINT NOT NULL CHECK (expected_slot_revision >= 0),
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending','approved','rejected','superseded')),
    submitted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reviewer_user_id BIGINT REFERENCES users(id) ON DELETE RESTRICT,
    decided_at TIMESTAMPTZ,
    reason TEXT CHECK (char_length(reason) <= 2000),
    CHECK ((state IN ('approved','rejected')) =
           (reviewer_user_id IS NOT NULL AND decided_at IS NOT NULL)),
    CHECK (state <> 'rejected' OR NULLIF(btrim(reason), '') IS NOT NULL)
);
CREATE UNIQUE INDEX book_cover_one_pending_review
    ON book_cover_review_requests(book_id) WHERE state='pending';
CREATE INDEX book_cover_review_pending_order
    ON book_cover_review_requests(id DESC) WHERE state='pending';
CREATE INDEX book_cover_review_book_history
    ON book_cover_review_requests(book_id, id DESC);

-- Immutable history even if request state later changes.
CREATE TABLE book_cover_review_decisions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id BIGINT NOT NULL UNIQUE
        REFERENCES book_cover_review_requests(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    job_id BIGINT NOT NULL REFERENCES cover_variant_jobs(id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL CHECK (outcome IN ('approved','rejected')),
    reason TEXT,
    decided_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (outcome <> 'rejected' OR NULLIF(btrim(reason), '') IS NOT NULL)
);
CREATE FUNCTION refuse_cover_decision_change()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'cover moderation audit is append-only'
        USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER cover_decisions_append_only
    BEFORE UPDATE OR DELETE ON book_cover_review_decisions
    FOR EACH ROW EXECUTE FUNCTION refuse_cover_decision_change();

-- Defense in depth: a late worker, retry or other stale code cannot bypass
-- moderation by writing a different job directly into the public slot.
CREATE FUNCTION require_approved_cover_slot()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' AND NEW.job_id=OLD.job_id THEN
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM book_cover_review_requests r
        JOIN book_cover_review_decisions d ON d.request_id=r.id
        WHERE r.book_id=NEW.book_id AND r.job_id=NEW.job_id
          AND r.state='approved' AND d.outcome='approved'
    ) THEN
        RAISE EXCEPTION 'cover slot requires site moderator approval'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER cover_slots_require_approval
    BEFORE INSERT OR UPDATE ON book_cover_slots
    FOR EACH ROW EXECUTE FUNCTION require_approved_cover_slot();
