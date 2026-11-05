-- MOD-REG-01: separate registration approval from routine Book visibility.
-- Legacy active Books keep their old public behavior; never invent their reviewer.
CREATE TABLE book_registration_decisions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    reviewer_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL CHECK (outcome IN ('approved','rejected')),
    expected_visibility_revision BIGINT NOT NULL CHECK (expected_visibility_revision > 0),
    resulting_visibility_revision BIGINT NOT NULL
        CHECK (resulting_visibility_revision=expected_visibility_revision+1),
    reason TEXT CHECK (char_length(reason) <= 2000),
    decided_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (outcome <> 'rejected' OR NULLIF(btrim(reason),'') IS NOT NULL),
    UNIQUE (book_id,expected_visibility_revision)
);
CREATE UNIQUE INDEX book_first_approved_registration
    ON book_registration_decisions(book_id) WHERE outcome='approved';
CREATE INDEX book_registration_decisions_book_history
    ON book_registration_decisions(book_id,id DESC);

ALTER TABLE books
    ADD COLUMN registration_requires_review BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN registration_approval_decision_id BIGINT
       REFERENCES book_registration_decisions(id) ON DELETE RESTRICT;
-- Historical pending Books do not acquire an invented approval.
UPDATE books SET registration_requires_review=TRUE
    WHERE visibility_status='pending_moderation';

CREATE FUNCTION verify_registration_approval_on_book()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.registration_approval_decision_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM book_registration_decisions d
        WHERE d.id=NEW.registration_approval_decision_id
          AND d.book_id=NEW.id AND d.outcome='approved'
    ) THEN
        RAISE EXCEPTION 'registration approval does not belong to Book'
            USING ERRCODE='23514';
    END IF;
    IF NEW.registration_requires_review
       AND NEW.visibility_status='active'
       AND NEW.registration_approval_decision_id IS NULL THEN
        RAISE EXCEPTION 'public Book activation requires site registration approval'
            USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER books_require_registration_approval
    BEFORE INSERT OR UPDATE OF visibility_status,registration_requires_review,registration_approval_decision_id
    ON books FOR EACH ROW EXECUTE FUNCTION verify_registration_approval_on_book();

CREATE FUNCTION registration_decision_immutable()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'book registration decision is immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER book_registration_decisions_append_only
    BEFORE UPDATE OR DELETE ON book_registration_decisions
    FOR EACH ROW EXECUTE FUNCTION registration_decision_immutable();
