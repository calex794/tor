-- I-reviewed FLOW-03. C owns canonical read/action/activity evidence; F owns
-- consented book-visit cohorts. Existing REC attribution and outbox are unchanged.
CREATE VIEW reader_flow_consent WITH (security_barrier=true) AS
SELECT s.user_id,s.subject_id,s.flow_epoch,s.flow_enabled_at
FROM rec_subjects s
WHERE s.enabled AND s.flow_epoch IS NOT NULL
  AND EXISTS(SELECT 1 FROM users u WHERE u.id=s.user_id AND u.account_state='active');

CREATE TABLE reader_flow_book_actions (
    event_id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    kind smallint NOT NULL CHECK(kind IN (1,2)),
    occurred_at timestamptz NOT NULL,
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE
);
CREATE INDEX reader_flow_actions_scope ON reader_flow_book_actions(subject_id,flow_epoch);
CREATE INDEX reader_flow_actions_expiry ON reader_flow_book_actions(occurred_at,event_id);

CREATE TABLE reader_flow_activity_days (
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    activity_day date NOT NULL,
    first_seen_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,flow_epoch,book_id,activity_day),
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE,
    CHECK(activity_day=(first_seen_at AT TIME ZONE 'UTC')::date)
);
CREATE INDEX reader_flow_activity_expiry ON reader_flow_activity_days(first_seen_at);

-- Approved C read ports. There is no browser-supplied fact or second outbox.
CREATE VIEW reader_flow_verified_book_actions WITH (security_barrier=true) AS
SELECT event_id,subject_id,flow_epoch,book_id,kind,occurred_at
FROM reader_flow_book_actions;
CREATE VIEW reader_flow_verified_activity_days WITH (security_barrier=true) AS
SELECT subject_id,flow_epoch,book_id,activity_day,first_seen_at
FROM reader_flow_activity_days;

-- Only the real INSERT transition produces a proof. An existing bookmark may
-- still pin newly released free chapters through its unchanged upsert path.
CREATE FUNCTION reader_flow_capture_book_action() RETURNS trigger
LANGUAGE plpgsql
SET lock_timeout='25ms'
AS $$
DECLARE
    current_scope record;
    action_kind smallint;
BEGIN
    IF current_setting('tor.flow_capture_enabled',true) IS DISTINCT FROM '1' THEN
        RETURN NULL;
    END IF;
    action_kind := CASE TG_TABLE_NAME
        WHEN 'reader_library_items' THEN 1
        WHEN 'reader_book_follows' THEN 2
        ELSE NULL END;
    IF action_kind IS NULL THEN
        RETURN NULL;
    END IF;
    BEGIN
        SELECT subject_id,flow_epoch INTO current_scope
        FROM reader_flow_consent
        WHERE user_id=NEW.user_id AND flow_enabled_at<=NEW.created_at
        FOR UPDATE;
        IF NOT FOUND THEN
            RETURN NULL;
        END IF;
        INSERT INTO reader_flow_book_actions
            (subject_id,flow_epoch,book_id,kind,occurred_at)
        VALUES(current_scope.subject_id,current_scope.flow_epoch,
               NEW.book_id,action_kind,NEW.created_at);
    EXCEPTION WHEN OTHERS THEN
        -- Only optional proof writes are rolled back. The C command reads this
        -- transaction-local signal after the statement; no blocking error-log
        -- table write or identifying SQL diagnostic runs in this handler.
        PERFORM set_config('tor.flow_capture_failed','1',true);
    END;
    RETURN NULL;
END
$$;
CREATE TRIGGER reader_flow_capture_library_save
    AFTER INSERT ON reader_library_items
    FOR EACH ROW EXECUTE FUNCTION reader_flow_capture_book_action();
CREATE TRIGGER reader_flow_capture_book_follow
    AFTER INSERT ON reader_book_follows
    FOR EACH ROW EXECUTE FUNCTION reader_flow_capture_book_action();

-- The historical evidence clauses match C's accepted audience report. Later
-- book visibility or purchase revocation must not rewrite an authorized fact.
-- first_for_book remains lifetime-first metadata, never an eligibility gate.
CREATE VIEW reader_flow_verified_read_events WITH (security_barrier=true) AS
SELECT c.source_event_id AS event_id,c.user_id,c.book_id,c.chapter_id,
       c.first_version_id AS version_id,c.purchase_id,c.started_at AS occurred_at,
       0::smallint AS kind,
       NOT EXISTS(SELECT 1 FROM reader_chapter_starts old
           WHERE old.user_id=c.user_id AND old.book_id=c.book_id
             AND (old.started_at,old.source_event_id)<(c.started_at,c.source_event_id))
       AND NOT EXISTS(SELECT 1 FROM reader_chapter_completions done
           WHERE done.user_id=c.user_id AND done.book_id=c.book_id
             AND done.completed_at<c.started_at) AS first_for_book
FROM reader_chapter_starts c
JOIN outbox_events e ON e.id=c.source_event_id
WHERE e.event_type='ChapterStartedV1' AND e.schema_version=1
  AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
  AND e.payload->>'user_id'=c.user_id::text
  AND e.payload->>'book_id'=c.book_id::text
  AND e.payload->>'chapter_id'=c.chapter_id::text
  AND e.payload->>'version_id'=c.first_version_id::text
  AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-start'
        AND NOT(e.payload ? 'purchase_id'))
    OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-start'
        AND e.payload->>'purchase_id'=c.purchase_id::text))
UNION ALL
SELECT c.source_event_id,c.user_id,c.book_id,c.chapter_id,
       c.first_version_id,c.purchase_id,c.completed_at,1::smallint,
       NOT EXISTS(SELECT 1 FROM reader_chapter_completions old
           WHERE old.user_id=c.user_id AND old.book_id=c.book_id
             AND (old.completed_at,old.source_event_id)<(c.completed_at,c.source_event_id))
FROM reader_chapter_completions c
JOIN outbox_events e ON e.id=c.source_event_id
WHERE e.event_type='ChapterCompletedV1' AND e.schema_version=1
  AND e.aggregate_kind='chapter' AND e.aggregate_id=c.chapter_id
  AND e.payload->>'user_id'=c.user_id::text
  AND e.payload->>'book_id'=c.book_id::text
  AND e.payload->>'chapter_id'=c.chapter_id::text
  AND e.payload->>'version_id'=c.first_version_id::text
  AND ((c.purchase_id IS NULL AND e.payload->>'authority'='C-verified-free-position'
        AND NOT(e.payload ? 'purchase_id'))
    OR (c.purchase_id IS NOT NULL AND e.payload->>'authority'='C-verified-purchased-position'
        AND e.payload->>'purchase_id'=c.purchase_id::text));

-- Internal navigation metadata comes from authorized successful handlers.
-- Proof references are not FKs: normal REC/raw expiry cannot erase FLOW facts.
ALTER TABLE flow_visit_receipts
    ADD COLUMN book_id bigint REFERENCES books(id) ON DELETE CASCADE,
    ADD COLUMN book_kind smallint CHECK(book_kind IN (0,1)),
    ADD COLUMN rec_assignment uuid,
    ADD COLUMN organic_touch uuid,
    ADD COLUMN discovery_kind smallint NOT NULL DEFAULT 3 CHECK(discovery_kind BETWEEN 0 AND 3),
    ADD COLUMN funnel_processed boolean NOT NULL DEFAULT false,
    ADD COLUMN cohort_id uuid,
    ADD CONSTRAINT flow_receipt_book_pair CHECK((book_id IS NULL)=(book_kind IS NULL)),
    ADD CONSTRAINT flow_receipt_book_surface CHECK(
        book_kind IS NULL OR (book_kind=0 AND surface=2) OR (book_kind=1 AND surface=3)),
    ADD CONSTRAINT flow_receipt_proof_book CHECK(
        book_id IS NOT NULL OR (rec_assignment IS NULL AND organic_touch IS NULL AND discovery_kind=3));
CREATE INDEX flow_receipt_pending_book
    ON flow_visit_receipts(observed_at,event_id)
    WHERE book_id IS NOT NULL AND NOT funnel_processed;

CREATE TABLE flow_book_cohorts (
    id uuid PRIMARY KEY,
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    visit_id uuid NOT NULL,
    visit_started_at timestamptz NOT NULL,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    first_entry_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    acquisition_hint smallint NOT NULL CHECK(acquisition_hint IN (0,1,3)),
    discovery_kind smallint NOT NULL CHECK(discovery_kind BETWEEN 0 AND 3),
    has_detail boolean NOT NULL DEFAULT false,
    has_reader boolean NOT NULL DEFAULT false,
    started boolean NOT NULL DEFAULT false,
    completed boolean NOT NULL DEFAULT false,
    saved boolean NOT NULL DEFAULT false,
    followed boolean NOT NULL DEFAULT false,
    new_reader boolean NOT NULL DEFAULT false,
    first_start_at timestamptz,
    first_start_event_id uuid UNIQUE,
    first_completion_event_id uuid UNIQUE,
    saved_event_id uuid UNIQUE,
    followed_event_id uuid UNIQUE,
    d7_state smallint NOT NULL DEFAULT 0 CHECK(d7_state BETWEEN 0 AND 3),
    finalized_at timestamptz,
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE,
    UNIQUE(subject_id,flow_epoch,visit_id,book_id),
    UNIQUE(id,subject_id,flow_epoch,book_id),
    CHECK(first_entry_at>=visit_started_at),
    CHECK(expires_at>first_entry_at AND expires_at<=visit_started_at+interval '30 days'),
    CHECK(has_detail OR has_reader),
    CHECK((first_start_at IS NOT NULL)=started),
    CHECK((first_start_event_id IS NOT NULL)=started),
    CHECK((first_completion_event_id IS NOT NULL)=completed),
    CHECK((saved_event_id IS NOT NULL)=saved),
    CHECK((followed_event_id IS NOT NULL)=followed),
    CHECK(NOT completed OR started),
    CHECK(NOT new_reader OR started),
    CHECK(d7_state=0 OR started),
    CHECK(first_start_at IS NULL OR first_start_at>=first_entry_at),
    CHECK(finalized_at IS NULL OR d7_state<>1)
);
CREATE INDEX flow_book_cohort_expiry ON flow_book_cohorts(expires_at,id);
CREATE INDEX flow_book_cohort_period ON flow_book_cohorts(first_entry_at,book_id);
CREATE INDEX flow_book_cohort_book ON flow_book_cohorts(book_id);

CREATE TABLE flow_book_return_anchors (
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    book_id bigint NOT NULL,
    cohort_id uuid NOT NULL UNIQUE,
    started_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY(subject_id,flow_epoch,book_id),
    FOREIGN KEY(cohort_id,subject_id,flow_epoch,book_id)
        REFERENCES flow_book_cohorts(id,subject_id,flow_epoch,book_id) ON DELETE CASCADE,
    CHECK(expires_at>started_at)
);
CREATE INDEX flow_book_return_anchor_expiry ON flow_book_return_anchors(expires_at);

CREATE TABLE flow_book_action_receipts (
    event_id uuid PRIMARY KEY,
    subject_id uuid NOT NULL,
    flow_epoch uuid NOT NULL,
    occurred_at timestamptz NOT NULL,
    FOREIGN KEY(subject_id,flow_epoch) REFERENCES rec_subjects(subject_id,flow_epoch)
        ON DELETE CASCADE
);
CREATE INDEX flow_book_action_receipt_scope ON flow_book_action_receipts(subject_id,flow_epoch);
CREATE INDEX flow_book_action_receipt_expiry ON flow_book_action_receipts(occurred_at,event_id);

CREATE TABLE flow_book_daily (
    cohort_day date NOT NULL,
    book_id bigint NOT NULL REFERENCES books(id) ON DELETE CASCADE,
    acquisition_hint smallint NOT NULL CHECK(acquisition_hint IN (0,1,3)),
    discovery_kind smallint NOT NULL CHECK(discovery_kind BETWEEN 0 AND 3),
    cohorts bigint NOT NULL CHECK(cohorts>=0),
    detail_opens bigint NOT NULL CHECK(detail_opens>=0 AND detail_opens<=cohorts),
    reader_opens bigint NOT NULL CHECK(reader_opens>=0 AND reader_opens<=cohorts),
    starts bigint NOT NULL CHECK(starts>=0 AND starts<=cohorts),
    completions bigint NOT NULL CHECK(completions>=0 AND completions<=starts),
    saves bigint NOT NULL CHECK(saves>=0 AND saves<=cohorts),
    follows bigint NOT NULL CHECK(follows>=0 AND follows<=cohorts),
    new_readers bigint NOT NULL CHECK(new_readers>=0 AND new_readers<=starts),
    d7_anchors bigint NOT NULL CHECK(d7_anchors>=0 AND d7_anchors<=starts),
    d7_returned bigint NOT NULL CHECK(d7_returned>=0 AND d7_returned<=d7_anchors),
    d7_unknown bigint NOT NULL CHECK(d7_unknown>=0 AND d7_unknown<=d7_anchors),
    PRIMARY KEY(cohort_day,book_id,acquisition_hint,discovery_kind),
    CHECK(d7_returned+d7_unknown=d7_anchors)
);
CREATE INDEX flow_book_daily_book ON flow_book_daily(book_id,cohort_day);
CREATE TABLE flow_book_worker_health (
    singleton boolean PRIMARY KEY CHECK(singleton),
    last_success timestamptz,
    last_collecting boolean NOT NULL,
    collecting_since timestamptz,
    last_tick timestamptz NOT NULL DEFAULT now(),
    backlog boolean NOT NULL DEFAULT true,
    CHECK(last_collecting OR collecting_since IS NULL)
);

-- Existing FLOW-02 erasure stays unchanged. This additive hook runs in the
-- same locked subject transaction and retracts only retained finalized inputs.
CREATE FUNCTION flow_book_erase_scope(p_subject uuid,p_epoch uuid) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE old_bucket record;
BEGIN
    IF p_epoch IS NULL THEN
        RETURN;
    END IF;
    FOR old_bucket IN
        SELECT (first_entry_at AT TIME ZONE 'UTC')::date AS cohort_day,
               book_id,acquisition_hint,discovery_kind,count(*) AS cohorts,
               count(*) FILTER(WHERE has_detail) AS detail_opens,
               count(*) FILTER(WHERE has_reader) AS reader_opens,
               count(*) FILTER(WHERE started) AS starts,
               count(*) FILTER(WHERE completed) AS completions,
               count(*) FILTER(WHERE saved) AS saves,
               count(*) FILTER(WHERE followed) AS follows,
               count(*) FILTER(WHERE new_reader) AS new_readers,
               count(*) FILTER(WHERE d7_state>0) AS d7_anchors,
               count(*) FILTER(WHERE d7_state=2) AS d7_returned,
               count(*) FILTER(WHERE d7_state=3) AS d7_unknown
        FROM flow_book_cohorts
        WHERE subject_id=p_subject AND flow_epoch=p_epoch AND finalized_at IS NOT NULL
        GROUP BY 1,2,3,4 ORDER BY 1,2,3,4
    LOOP
        UPDATE flow_book_daily d SET
            cohorts=d.cohorts-old_bucket.cohorts,
            detail_opens=d.detail_opens-old_bucket.detail_opens,
            reader_opens=d.reader_opens-old_bucket.reader_opens,
            starts=d.starts-old_bucket.starts,
            completions=d.completions-old_bucket.completions,
            saves=d.saves-old_bucket.saves,
            follows=d.follows-old_bucket.follows,
            new_readers=d.new_readers-old_bucket.new_readers,
            d7_anchors=d.d7_anchors-old_bucket.d7_anchors,
            d7_returned=d.d7_returned-old_bucket.d7_returned,
            d7_unknown=d.d7_unknown-old_bucket.d7_unknown
        WHERE d.cohort_day=old_bucket.cohort_day AND d.book_id=old_bucket.book_id
          AND d.acquisition_hint=old_bucket.acquisition_hint AND d.discovery_kind=old_bucket.discovery_kind;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'FLOW book aggregate source is unavailable' USING ERRCODE='23514';
        END IF;
    END LOOP;
    DELETE FROM flow_book_cohorts WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_book_action_receipts WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM reader_flow_book_actions WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM reader_flow_activity_days WHERE subject_id=p_subject AND flow_epoch=p_epoch;
END
$$;
CREATE FUNCTION flow_book_erase_replaced_scope() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        PERFORM flow_book_erase_scope(OLD.subject_id,OLD.flow_epoch);
        RETURN OLD;
    END IF;
    IF OLD.subject_id IS DISTINCT FROM NEW.subject_id
        OR OLD.flow_epoch IS DISTINCT FROM NEW.flow_epoch THEN
        PERFORM flow_book_erase_scope(OLD.subject_id,OLD.flow_epoch);
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER flow_book_erase_replaced_scope
    BEFORE UPDATE OF subject_id,flow_epoch ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION flow_book_erase_replaced_scope();
CREATE TRIGGER flow_book_erase_deleted_scope
    BEFORE DELETE ON rec_subjects
    FOR EACH ROW EXECUTE FUNCTION flow_book_erase_replaced_scope();

COMMENT ON VIEW reader_flow_consent IS
    'F-approved row-lockable C collection port: active account, explicit FLOW epoch, no implicit opt-in from REC.';
COMMENT ON TABLE reader_flow_book_actions IS
    'C optional committed absent-to-present save/follow proofs; raw30d; no backfill, raw URL, token, IP or second outbox.';
COMMENT ON TABLE reader_flow_activity_days IS
    'C positive authorized progress only, one row per FLOW scope/book/UTC day; raw30d; GET is not reading.';
COMMENT ON TABLE flow_book_cohorts IS
    'F first authorized book-entry cohort per visit/book; origin expires at most30d after visit start; canonical C IDs only; no identity in reports.';
COMMENT ON TABLE flow_book_return_anchors IS
    'Retained D7 anchor only: earliest historical C start after FLOW enable must map to this cohort; no rollover or dateless subject/book tombstone.';
COMMENT ON TABLE flow_book_daily IS
    'Anonymous book/cohort-day/source stage counts, <=180d; book-visit cohorts are not unique people; D7 absence remains unknown.';
