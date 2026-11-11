-- F-SURPRISE-04B / I integration. Additive, opt-in, no raw interaction log.
-- One bounded receipt/account/run; deletion unlinks identity, not the ITT denominator.
CREATE SEQUENCE surprise_capture_fault_seq;
CREATE FUNCTION surprise_capture_faults() RETURNS bigint LANGUAGE sql VOLATILE AS $$
    SELECT CASE WHEN is_called THEN last_value ELSE 0 END FROM surprise_capture_fault_seq
$$;

CREATE TABLE surprise_experiment_runs (
    id bigserial PRIMARY KEY,
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    enrollment_ends_at timestamptz NOT NULL DEFAULT clock_timestamp()+interval '336 hours',
    ends_at timestamptz NOT NULL DEFAULT clock_timestamp()+interval '528 hours',
    last_checked_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    probability_per_mille smallint NOT NULL CHECK(probability_per_mille BETWEEN 0 AND 1000),
    state text NOT NULL DEFAULT 'running' CHECK(state IN ('running','stopped','completed')),
    decision text NOT NULL DEFAULT 'collecting',
    faults_start bigint NOT NULL DEFAULT surprise_capture_faults(),
    faults_end bigint,
    closed_at timestamptz,
    CHECK(started_at<enrollment_ends_at AND enrollment_ends_at<ends_at)
);
CREATE UNIQUE INDEX surprise_one_running_experiment ON surprise_experiment_runs((true))
    WHERE state='running';

CREATE TABLE surprise_experiment_participants (
    id bigserial PRIMARY KEY,
    run_id bigint NOT NULL REFERENCES surprise_experiment_runs(id),
    user_id bigint REFERENCES users(id) ON DELETE SET NULL,
    arm text NOT NULL CHECK(arm IN ('control','gentle','standard')),
    enrolled_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    baseline_chapter_id bigint,
    prior_reader boolean NOT NULL DEFAULT false,
    meaningful_new_book boolean NOT NULL DEFAULT false,
    current_completed boolean NOT NULL DEFAULT false,
    returned_d1 boolean NOT NULL DEFAULT false,
    returned_d7 boolean NOT NULL DEFAULT false,
    disabled boolean NOT NULL DEFAULT false,
    CONSTRAINT surprise_participant_once UNIQUE(run_id,user_id)
);
CREATE INDEX surprise_participant_actor ON surprise_experiment_participants(user_id,run_id);
CREATE INDEX surprise_participant_retention ON surprise_experiment_participants(enrolled_at,id);

CREATE TABLE surprise_experiment_totals (
    run_id bigint NOT NULL REFERENCES surprise_experiment_runs(id),
    arm text NOT NULL CHECK(arm IN ('control','gentle','standard')),
    enrolled bigint NOT NULL,
    mature bigint NOT NULL,
    meaningful bigint NOT NULL,
    baseline_eligible bigint NOT NULL,
    current_completed bigint NOT NULL,
    returned_d1 bigint NOT NULL,
    returned_d7 bigint NOT NULL,
    disabled bigint NOT NULL,
    prior_readers bigint NOT NULL,
    measured_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(run_id,arm)
);
ALTER TABLE surprise_runtime_policy ADD COLUMN experiment_run_id bigint
    REFERENCES surprise_experiment_runs(id);
ALTER TABLE surprise_daily_decisions ADD COLUMN experiment_run_id bigint
    REFERENCES surprise_experiment_runs(id);
-- An old 04A pilot has no fixed cohort and cannot silently become a 04B trial.
UPDATE surprise_runtime_policy SET enabled=enabled AND NOT experiment_enabled,
    experiment_enabled=false;

CREATE FUNCTION surprise_refresh_totals(p_run bigint) RETURNS void LANGUAGE sql AS $$
    INSERT INTO surprise_experiment_totals
      (run_id,arm,enrolled,mature,meaningful,baseline_eligible,current_completed,
       returned_d1,returned_d7,disabled,prior_readers,measured_at)
    SELECT p_run,a.arm,count(p.id),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours'),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours' AND p.meaningful_new_book),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours' AND p.baseline_chapter_id IS NOT NULL),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours' AND p.baseline_chapter_id IS NOT NULL AND p.current_completed),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours' AND p.returned_d1),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours' AND p.returned_d7),
      count(p.id) FILTER(WHERE p.enrolled_at<=transaction_timestamp()-interval '192 hours' AND p.disabled),
      count(p.id) FILTER(WHERE p.prior_reader),transaction_timestamp()
    FROM unnest(ARRAY['control','gentle','standard']) a(arm)
    LEFT JOIN surprise_experiment_participants p ON p.run_id=p_run AND p.arm=a.arm
    GROUP BY a.arm
    ON CONFLICT(run_id,arm) DO UPDATE SET enrolled=excluded.enrolled,mature=excluded.mature,
      meaningful=excluded.meaningful,baseline_eligible=excluded.baseline_eligible,
      current_completed=excluded.current_completed,returned_d1=excluded.returned_d1,
      returned_d7=excluded.returned_d7,disabled=excluded.disabled,
      prior_readers=excluded.prior_readers,measured_at=excluded.measured_at
$$;

CREATE FUNCTION surprise_experiment_policy_transition() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE active_id bigint;
BEGIN
    IF OLD.experiment_enabled AND
       (NOT NEW.experiment_enabled OR NOT NEW.enabled OR
        NEW.probability_per_mille<>OLD.probability_per_mille) THEN
        UPDATE surprise_experiment_runs SET state='stopped',decision='operator_stopped',
          closed_at=clock_timestamp(),faults_end=surprise_capture_faults()
          WHERE id=OLD.experiment_run_id AND state='running';
        IF FOUND THEN PERFORM surprise_refresh_totals(OLD.experiment_run_id); END IF;
        NEW.enabled:=false;
        NEW.experiment_enabled:=false;
    ELSIF NEW.experiment_enabled AND NOT OLD.experiment_enabled THEN
        IF NOT NEW.enabled THEN
            NEW.experiment_enabled:=false;
            RETURN NEW;
        END IF;
        INSERT INTO surprise_experiment_runs(probability_per_mille)
          VALUES(NEW.probability_per_mille) RETURNING id INTO active_id;
        NEW.experiment_run_id:=active_id;
        PERFORM surprise_refresh_totals(active_id);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER surprise_experiment_policy_transition
    BEFORE UPDATE ON surprise_runtime_policy FOR EACH ROW
    EXECUTE FUNCTION surprise_experiment_policy_transition();

-- I-owned read projection: C remains authoritative for starts, positions and completions.
CREATE FUNCTION surprise_enroll(p_user bigint,p_candidate text,p_day bigint,p_run bigint)
RETURNS TABLE(run_id bigint,arm text,probability smallint) LANGUAGE plpgsql AS $$
DECLARE r surprise_experiment_runs%ROWTYPE; selected_arm text; at_time timestamptz;
BEGIN
    IF p_candidate IS NULL OR p_candidate NOT IN ('control','gentle','standard') THEN RETURN; END IF;
    SELECT e.* INTO r FROM surprise_experiment_runs e
      JOIN surprise_runtime_policy policy ON policy.experiment_run_id=e.id
      WHERE policy.singleton AND policy.enabled AND policy.experiment_enabled
        AND e.id=p_run AND e.state='running' AND clock_timestamp()<e.ends_at
        AND e.last_checked_at>=clock_timestamp()-interval '2 minutes'
        AND e.faults_start=surprise_capture_faults() FOR SHARE OF e;
    IF NOT FOUND THEN RETURN; END IF;
    IF EXISTS(SELECT 1 FROM surprise_daily_decisions d WHERE d.user_id=p_user
       AND d.utc_day=p_day AND d.experiment_run_id IS DISTINCT FROM r.id) THEN RETURN; END IF;
    SELECT p.arm INTO selected_arm FROM surprise_experiment_participants p
      WHERE p.run_id=r.id AND p.user_id=p_user
        AND clock_timestamp()<p.enrolled_at+interval '192 hours';
    IF NOT FOUND THEN
        IF clock_timestamp()>=r.enrollment_ends_at OR EXISTS(
            SELECT 1 FROM surprise_experiment_participants p
            WHERE p.run_id=r.id AND p.user_id=p_user) THEN RETURN; END IF;
        at_time:=clock_timestamp();
        INSERT INTO surprise_experiment_participants
          (run_id,user_id,arm,enrolled_at,baseline_chapter_id,prior_reader)
        SELECT r.id,p_user,p_candidate,at_time,
          (SELECT recent.chapter_id FROM (
             SELECT chapter_id,updated_at FROM reader_book_positions
               WHERE user_id=p_user AND (block_index>1 OR char_offset>0)
             UNION ALL
             SELECT chapter_id,updated_at FROM reader_purchased_positions
               WHERE user_id=p_user AND (block_index>1 OR char_offset>0)
           ) recent WHERE recent.updated_at>=at_time-interval '24 hours'
             AND NOT EXISTS(SELECT 1 FROM reader_chapter_completions c
               WHERE c.user_id=p_user AND c.chapter_id=recent.chapter_id)
           ORDER BY recent.updated_at DESC,recent.chapter_id LIMIT 1),
          EXISTS(SELECT 1 FROM reader_chapter_starts s WHERE s.user_id=p_user)
            OR EXISTS(SELECT 1 FROM reader_chapter_completions c WHERE c.user_id=p_user)
        ON CONFLICT ON CONSTRAINT surprise_participant_once DO NOTHING;
        SELECT p.arm INTO selected_arm FROM surprise_experiment_participants p
          WHERE p.run_id=r.id AND p.user_id=p_user;
    END IF;
    RETURN QUERY SELECT r.id,selected_arm,r.probability_per_mille;
END $$;

CREATE FUNCTION surprise_delivery_allowed(p_user bigint,p_day bigint)
RETURNS boolean LANGUAGE sql VOLATILE AS $$
    SELECT COALESCE((SELECT policy.enabled AND CASE WHEN policy.experiment_enabled THEN
        d.experiment_run_id=policy.experiment_run_id AND d.experiment_arm IN ('gentle','standard')
        AND EXISTS(SELECT 1 FROM surprise_experiment_runs e
          JOIN surprise_experiment_participants p ON p.run_id=e.id AND p.user_id=p_user
          WHERE e.id=d.experiment_run_id AND p.arm=d.experiment_arm AND e.state='running'
            AND e.faults_start=surprise_capture_faults() AND clock_timestamp()<e.ends_at
            AND e.last_checked_at>=clock_timestamp()-interval '2 minutes'
            AND clock_timestamp()<p.enrolled_at+interval '192 hours')
      ELSE d.experiment_arm IS NULL END
      FROM surprise_daily_decisions d CROSS JOIN surprise_runtime_policy policy
      WHERE policy.singleton AND d.user_id=p_user AND d.utc_day=p_day),false)
$$;

COMMENT ON TABLE surprise_experiment_participants IS
  'F-SURPRISE-04B: bounded ITT receipt, no URL/click/device/book history. Purged after 30 days; account deletion unlinks user_id.';
COMMENT ON TABLE surprise_experiment_totals IS
  'Three aggregate rows/run retained after participant deletion. Mature population uses the same full eight-day observation in every arm.';
