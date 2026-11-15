-- I-reviewed F-owned visit observations. Existing REC consent never opts in.
-- RIL's anonymous rollups and its in-memory identity policy are unchanged.
ALTER TABLE rec_subjects
    ADD COLUMN flow_epoch uuid,
    ADD COLUMN flow_enabled_at timestamptz,
    ADD CONSTRAINT rec_flow_consent_pair
        CHECK ((flow_epoch IS NULL)=(flow_enabled_at IS NULL)),
    ADD CONSTRAINT rec_flow_requires_consent
        CHECK (flow_epoch IS NULL OR enabled),
    ADD CONSTRAINT rec_flow_scope UNIQUE(subject_id,flow_epoch);

CREATE TABLE flow_visits (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    started_at timestamptz NOT NULL,
    last_at timestamptz NOT NULL,
    first_event_id uuid NOT NULL,
    last_event_id uuid NOT NULL,
    landing_surface smallint NOT NULL CHECK(landing_surface BETWEEN 0 AND 4),
    last_surface smallint NOT NULL CHECK(last_surface BETWEEN 0 AND 4),
    acquisition_hint smallint NOT NULL CHECK(acquisition_hint BETWEEN 0 AND 3),
    pageviews bigint NOT NULL CHECK(pageviews>0),
    finalized_at timestamptz,
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE,
    CHECK(last_at>=started_at),
    CHECK(finalized_at IS NULL OR finalized_at>=last_at)
);
CREATE INDEX flow_visits_scope_activity
    ON flow_visits(subject_id,flow_epoch,last_at DESC);
CREATE INDEX flow_visits_finalization
    ON flow_visits(last_at,subject_id) WHERE finalized_at IS NULL;
CREATE INDEX flow_visits_expiry ON flow_visits(started_at);

-- Event UUIDs are generated internally; no browser token or request URL is saved.
CREATE TABLE flow_visit_receipts (
    event_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    observed_at timestamptz NOT NULL,
    surface smallint NOT NULL CHECK(surface BETWEEN 0 AND 4),
    acquisition_hint smallint NOT NULL CHECK(acquisition_hint BETWEEN 0 AND 3),
    continuation_unknown boolean NOT NULL DEFAULT false,
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE
);
CREATE INDEX flow_visit_receipts_scope ON flow_visit_receipts(subject_id,flow_epoch);
CREATE INDEX flow_visit_receipts_expiry ON flow_visit_receipts(observed_at);

-- A visit that spans raw retention must not become a fictitious new start.
-- Only its latest observation survives, for an idle window plus queue delay; there
-- is no original date, source, surface or persistent visit identifier here.
CREATE TABLE flow_visit_continuations (
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    last_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,flow_epoch),
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE
);
CREATE INDEX flow_visit_continuations_expiry ON flow_visit_continuations(last_at);

-- One observation per opted-in account/UTC date, never a sum of daily uniques.
-- Retained only for today and the previous 29 UTC dates.
CREATE TABLE flow_visit_account_days (
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    bucket_date date NOT NULL,
    PRIMARY KEY(subject_id,flow_epoch,bucket_date),
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE
);
CREATE INDEX flow_visit_account_days_expiry ON flow_visit_account_days(bucket_date);

-- Completed-visit totals contain no subject, account, book or event identifiers.
CREATE TABLE flow_visit_daily (
    cohort_day date NOT NULL,
    acquisition_hint smallint NOT NULL CHECK(acquisition_hint BETWEEN 0 AND 3),
    landing_surface smallint NOT NULL CHECK(landing_surface BETWEEN 0 AND 4),
    last_surface smallint NOT NULL CHECK(last_surface BETWEEN 0 AND 4),
    visits bigint NOT NULL CHECK(visits>=0),
    pageviews bigint NOT NULL CHECK(pageviews>=visits),
    observed_seconds bigint NOT NULL CHECK(observed_seconds>=0),
    PRIMARY KEY(cohort_day,acquisition_hint,landing_surface,last_surface)
);

-- All callers serialize on rec_subjects before touching visits or daily rows.
-- Retract only the contribution which still has an individual retained source;
-- the already anonymous historical totals cannot identify a revoked account.
CREATE FUNCTION flow_erase_scope(p_subject uuid,p_epoch uuid) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE old_bucket record;
BEGIN
    IF p_epoch IS NULL THEN
        RETURN;
    END IF;
    FOR old_bucket IN
        SELECT (started_at AT TIME ZONE 'UTC')::date AS cohort_day,
               acquisition_hint,landing_surface,last_surface,
               count(*) AS visits,sum(pageviews)::bigint AS pageviews,
               sum(floor(extract(epoch FROM (last_at-started_at)))::bigint)
                   AS observed_seconds
        FROM flow_visits
        WHERE subject_id=p_subject AND flow_epoch=p_epoch AND finalized_at IS NOT NULL
        GROUP BY 1,2,3,4 ORDER BY 1,2,3,4
    LOOP
        UPDATE flow_visit_daily d SET
            visits=d.visits-old_bucket.visits,
            pageviews=d.pageviews-old_bucket.pageviews,
            observed_seconds=d.observed_seconds-old_bucket.observed_seconds
        WHERE d.cohort_day=old_bucket.cohort_day
          AND d.acquisition_hint=old_bucket.acquisition_hint
          AND d.landing_surface=old_bucket.landing_surface
          AND d.last_surface=old_bucket.last_surface;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'FLOW aggregate source is unavailable'
                USING ERRCODE='23514';
        END IF;
    END LOOP;
    DELETE FROM flow_visits WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_visit_receipts WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_visit_account_days WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_visit_continuations WHERE subject_id=p_subject AND flow_epoch=p_epoch;
END
$$;

-- The hook also protects account deletion and direct consent-epoch rotation.
-- Explicit repository erasure calls the same function; the second call is empty.
CREATE FUNCTION flow_erase_replaced_scope() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        PERFORM flow_erase_scope(OLD.subject_id,OLD.flow_epoch);
        RETURN OLD;
    END IF;
    IF OLD.subject_id IS DISTINCT FROM NEW.subject_id
        OR OLD.flow_epoch IS DISTINCT FROM NEW.flow_epoch THEN
        PERFORM flow_erase_scope(OLD.subject_id,OLD.flow_epoch);
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER flow_erase_replaced_scope
    BEFORE UPDATE OF subject_id,flow_epoch ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION flow_erase_replaced_scope();
CREATE TRIGGER flow_erase_deleted_scope
    BEFORE DELETE ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION flow_erase_replaced_scope();

COMMENT ON TABLE flow_visits IS
    'F explicit FLOW opt-in scope: compact observed visits, <=30 days raw retention; no anonymous identity, raw URL, IP, browser token or text.';
COMMENT ON TABLE flow_visit_daily IS
    'F anonymous finalized visit counts by UTC start day and bounded surface/source hints, <=180 days; not unique people or verified external acquisition.';
