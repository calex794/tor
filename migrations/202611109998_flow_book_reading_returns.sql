-- I-reviewed C-owned bounded book observations. No historical backfill.
-- The separate coverage history starts only with its explicit collection flag.
CREATE VIEW reader_return_public_books WITH (security_barrier=true) AS
SELECT id,title FROM rec_public_books;
COMMENT ON VIEW reader_return_public_books IS
    'F-approved read-only C report port: current public/basic/general book visibility from rec_public_books; no duplicated visibility policy.';

CREATE TABLE reader_book_return_coverage (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at timestamptz NOT NULL,
    last_heartbeat_at timestamptz NOT NULL,
    ended_at timestamptz,
    CHECK(last_heartbeat_at>=started_at),
    CHECK(ended_at IS NULL OR ended_at>=started_at)
);
CREATE UNIQUE INDEX reader_book_return_one_active_coverage
    ON reader_book_return_coverage((true)) WHERE ended_at IS NULL;
CREATE INDEX reader_book_return_coverage_period
    ON reader_book_return_coverage(started_at);

-- This is the entire personal book observation: no chapter, position, raw
-- event history, title or permanent subject/book marker is retained here.
-- An expired episode may coexist briefly with its successor until the worker
-- atomically transfers and erases that exact (subject,book,cohort_day) tuple.
CREATE TABLE reader_book_return_subjects (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    cohort_day date NOT NULL,
    returned_d7 boolean NOT NULL DEFAULT false,
    returned_d30 boolean NOT NULL DEFAULT false,
    PRIMARY KEY(subject_id,book_id,cohort_day)
);
CREATE INDEX reader_book_return_subjects_due
    ON reader_book_return_subjects(cohort_day,subject_id,book_id);
CREATE INDEX reader_book_return_subjects_report
    ON reader_book_return_subjects(book_id,cohort_day);

-- No user/subject identifiers survive finalization. Each book's aggregate is
-- retained for 180 cohort days and still obeys live book visibility in HTTP.
CREATE TABLE reader_book_return_daily (
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    cohort_day date NOT NULL,
    day smallint NOT NULL CHECK(day IN (7,30)),
    enrolled bigint NOT NULL CHECK(enrolled>=0),
    eligible bigint NOT NULL CHECK(eligible>=0),
    returned bigint NOT NULL CHECK(returned>=0 AND returned<=eligible),
    unknown bigint NOT NULL CHECK(unknown>=0),
    PRIMARY KEY(book_id,cohort_day,day),
    CHECK(eligible+unknown=enrolled)
);
CREATE INDEX reader_book_return_daily_expiry ON reader_book_return_daily(cohort_day);

CREATE FUNCTION reader_book_return_erase_rotated_consent() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.subject_id IS DISTINCT FROM NEW.subject_id THEN
        DELETE FROM reader_book_return_subjects WHERE subject_id=OLD.subject_id;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER reader_book_return_erase_rotated_consent
    BEFORE UPDATE OF subject_id ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION reader_book_return_erase_rotated_consent();

COMMENT ON TABLE reader_book_return_subjects IS
    'C bounded book observations: book identifier, cohort date and D7/D30 flags until end of UTC D30 (at most 31 days); then exact tuple erased with no permanent subject/book marker.';
