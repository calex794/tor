-- I-reviewed C/F boundary: optional confirmed reading-day collection.
-- No historical progress, first-start fact, GET or purchase is backfilled.
CREATE VIEW reader_activity_consent WITH (security_barrier=true) AS
SELECT user_id,subject_id,enabled_at FROM rec_subjects WHERE enabled;

CREATE TABLE reader_return_coverage (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    started_at timestamptz NOT NULL,
    last_heartbeat_at timestamptz NOT NULL,
    ended_at timestamptz,
    CHECK(last_heartbeat_at>=started_at),
    CHECK(ended_at IS NULL OR ended_at>=started_at)
);
CREATE UNIQUE INDEX reader_return_one_active_coverage
    ON reader_return_coverage((true)) WHERE ended_at IS NULL;
CREATE INDEX reader_return_coverage_period ON reader_return_coverage(started_at);

-- A privacy epoch, not an account identifier in an administrative report.
-- Detailed book/day data expires after 30*24 hours from its first observation.
CREATE TABLE reader_activity_days (
    subject_id uuid NOT NULL REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    activity_day date NOT NULL,
    first_seen_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,book_id,activity_day),
    CHECK(activity_day=(first_seen_at AT TIME ZONE 'UTC')::date)
);
CREATE INDEX reader_activity_days_expiry ON reader_activity_days(first_seen_at);

-- Only this minimal state survives beyond the 30*24-hour journal boundary:
-- cohort day + three return flags + coverage reference, until D30 has ended.
-- At that point all temporal fields become NULL and only the enrollment bit
-- remains for this consent epoch. It prevents a later visit being called new.
CREATE TABLE reader_return_subjects (
    subject_id uuid PRIMARY KEY REFERENCES rec_subjects(subject_id) ON DELETE CASCADE,
    enrolled boolean NOT NULL DEFAULT true CHECK(enrolled),
    cohort_day date,
    coverage_epoch_id bigint REFERENCES reader_return_coverage(id),
    returned_d1 boolean,
    returned_d7 boolean,
    returned_d30 boolean,
    CHECK ((cohort_day IS NULL AND coverage_epoch_id IS NULL
              AND returned_d1 IS NULL AND returned_d7 IS NULL AND returned_d30 IS NULL)
        OR (cohort_day IS NOT NULL AND coverage_epoch_id IS NOT NULL
              AND returned_d1 IS NOT NULL AND returned_d7 IS NOT NULL AND returned_d30 IS NOT NULL))
);
CREATE INDEX reader_return_subjects_due ON reader_return_subjects(cohort_day)
    WHERE cohort_day IS NOT NULL;

-- All three horizons are transferred atomically when the D30 day is complete.
-- This table contains no subject/account/book/chapter identifiers; TTL 180 days.
CREATE TABLE reader_return_daily (
    cohort_day date NOT NULL,
    day smallint NOT NULL CHECK(day IN (1,7,30)),
    enrolled bigint NOT NULL CHECK(enrolled>=0),
    eligible bigint NOT NULL CHECK(eligible>=0),
    returned bigint NOT NULL CHECK(returned>=0 AND returned<=eligible),
    unknown bigint NOT NULL CHECK(unknown>=0),
    PRIMARY KEY(cohort_day,day),
    CHECK(eligible+unknown=enrolled)
);

-- F owns consent rotation. This approved C erasure hook executes inside that
-- same subject-row-locked transaction, before the FK target is replaced.
CREATE FUNCTION reader_return_erase_rotated_consent() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.subject_id IS DISTINCT FROM NEW.subject_id THEN
        DELETE FROM reader_activity_days WHERE subject_id=OLD.subject_id;
        DELETE FROM reader_return_subjects WHERE subject_id=OLD.subject_id;
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER reader_return_erase_rotated_consent
    BEFORE UPDATE OF subject_id ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION reader_return_erase_rotated_consent();

COMMENT ON TABLE reader_return_subjects IS
    'C confirmed-reading cohorts: minimal temporal state through UTC D30 (at most 31 calendar days), then a dateless enrollment bit until consent rotation/deletion.';
COMMENT ON VIEW reader_activity_consent IS
    'F-approved C collection port; FOR UPDATE serializes accepted C progress and anonymization with F opt-out/consent epoch rotation.';
