-- I-owned C -> F bridge. No additional consent and no new reading event log.
-- Existing C validation/entitlements remain the only authority. No public event endpoint.
-- Optional capture uses a 25 ms lock budget; failures invalidate the experiment,
-- never turn a missing measurement into a measured zero or break a valid C write.
CREATE FUNCTION surprise_capture_read_outcome() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE p surprise_experiment_participants%ROWTYPE; active_id bigint;
        at_time timestamptz; old_timeout text; verified boolean; is_new boolean;
BEGIN
    at_time:=clock_timestamp();
    IF TG_TABLE_NAME<>'reader_chapter_completions' THEN
        IF NEW.block_index=1 AND NEW.char_offset=0 THEN RETURN NEW; END IF;
        IF TG_OP='UPDATE' AND NEW.revision<=OLD.revision THEN RETURN NEW; END IF;
    END IF;
    old_timeout:=current_setting('lock_timeout');
    BEGIN
        PERFORM set_config('lock_timeout','25ms',true);
        SELECT e.id INTO active_id FROM surprise_experiment_runs e
          JOIN surprise_runtime_policy policy ON policy.experiment_run_id=e.id
          WHERE policy.singleton AND policy.enabled AND policy.experiment_enabled
            AND e.state='running' AND at_time<e.ends_at FOR SHARE OF e;
        IF active_id IS NOT NULL THEN
            SELECT x.* INTO p FROM surprise_experiment_participants x
              WHERE x.run_id=active_id AND x.user_id=NEW.user_id
                AND at_time>=x.enrolled_at AND at_time<x.enrolled_at+interval '8 days'
              FOR UPDATE;
            IF FOUND THEN
                IF TG_TABLE_NAME='reader_chapter_completions' THEN
                    -- Deferred trigger: the canonical completion and its outbox
                    -- receipt are both visible, even for the same modifying CTE.
                    SELECT EXISTS(SELECT 1 FROM outbox_events e
                      WHERE e.id=NEW.source_event_id AND e.event_type='ChapterCompletedV1'
                        AND e.schema_version=1 AND e.aggregate_kind='chapter'
                        AND e.aggregate_id=NEW.chapter_id
                        AND e.payload->>'user_id'=NEW.user_id::text
                        AND e.payload->>'book_id'=NEW.book_id::text
                        AND e.payload->>'chapter_id'=NEW.chapter_id::text
                        AND e.payload->>'version_id'=NEW.first_version_id::text
                        AND ((NEW.purchase_id IS NULL
                              AND e.payload->>'authority'='C-verified-free-position'
                              AND NOT(e.payload ? 'purchase_id'))
                          OR (NEW.purchase_id IS NOT NULL
                              AND e.payload->>'authority'='C-verified-purchased-position'
                              AND e.payload->>'purchase_id'=NEW.purchase_id::text))) INTO verified;
                    IF verified AND NEW.completed_at>=p.enrolled_at
                       AND at_time<p.enrolled_at+interval '7 days' THEN
                        SELECT NOT EXISTS(SELECT 1 FROM reader_chapter_starts s
                          WHERE s.user_id=NEW.user_id AND s.book_id=NEW.book_id
                            AND s.started_at<p.enrolled_at)
                          AND NOT EXISTS(SELECT 1 FROM reader_chapter_completions c
                            WHERE c.user_id=NEW.user_id AND c.book_id=NEW.book_id
                              AND c.completed_at<p.enrolled_at) INTO is_new;
                        UPDATE surprise_experiment_participants SET
                          meaningful_new_book=meaningful_new_book OR is_new,
                          current_completed=current_completed OR COALESCE(baseline_chapter_id=NEW.chapter_id,false)
                          WHERE id=p.id;
                    END IF;
                ELSE
                    UPDATE surprise_experiment_participants SET
                      returned_d1=returned_d1 OR (at_time>=enrolled_at+interval '1 day'
                        AND at_time<enrolled_at+interval '2 days'),
                      returned_d7=returned_d7 OR at_time>=enrolled_at+interval '7 days'
                      WHERE id=p.id;
                END IF;
            END IF;
        END IF;
        PERFORM set_config('lock_timeout',old_timeout,true);
    EXCEPTION WHEN OTHERS THEN
        -- Sequences do not wait for the participant/run row and survive rollback.
        -- PostgreSQL OTHERS intentionally does not swallow query_canceled.
        PERFORM nextval('surprise_capture_fault_seq');
        PERFORM set_config('lock_timeout',old_timeout,true);
    END;
    RETURN NEW;
END $$;
CREATE TRIGGER surprise_verified_free_activity AFTER INSERT OR UPDATE ON reader_book_positions
    FOR EACH ROW EXECUTE FUNCTION surprise_capture_read_outcome();
CREATE TRIGGER surprise_verified_paid_activity AFTER INSERT OR UPDATE ON reader_purchased_positions
    FOR EACH ROW EXECUTE FUNCTION surprise_capture_read_outcome();
CREATE CONSTRAINT TRIGGER surprise_verified_completion
    AFTER INSERT ON reader_chapter_completions DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION surprise_capture_read_outcome();

CREATE FUNCTION surprise_capture_disable() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE old_timeout text;
BEGIN
    IF NOT NEW.disabled THEN RETURN NEW; END IF;
    old_timeout:=current_setting('lock_timeout');
    BEGIN
        PERFORM set_config('lock_timeout','25ms',true);
        UPDATE surprise_experiment_participants p SET disabled=true
          FROM surprise_experiment_runs e
          WHERE p.run_id=e.id AND p.user_id=NEW.user_id AND e.state='running'
            AND clock_timestamp()<e.ends_at
            AND clock_timestamp()<p.enrolled_at+interval '8 days';
        PERFORM set_config('lock_timeout',old_timeout,true);
    EXCEPTION WHEN OTHERS THEN
        PERFORM nextval('surprise_capture_fault_seq');
        PERFORM set_config('lock_timeout',old_timeout,true);
    END;
    RETURN NEW;
END $$;
CREATE TRIGGER surprise_experiment_disable AFTER INSERT OR UPDATE OF disabled
    ON surprise_user_preferences FOR EACH ROW EXECUTE FUNCTION surprise_capture_disable();

CREATE FUNCTION surprise_purge_experiment_batch() RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE removed bigint; other_removed bigint;
BEGIN
    WITH old_rows AS (
      SELECT p.id FROM surprise_experiment_participants p
      JOIN surprise_experiment_runs e ON e.id=p.run_id
      WHERE p.enrolled_at<clock_timestamp()-interval '30 days' AND e.state<>'running'
      ORDER BY p.enrolled_at,p.id FOR UPDATE OF p SKIP LOCKED LIMIT 1000
    ) DELETE FROM surprise_experiment_participants WHERE id IN (SELECT id FROM old_rows);
    GET DIAGNOSTICS removed=ROW_COUNT;
    WITH old_rows AS (
      SELECT user_id,utc_day FROM surprise_daily_decisions
      WHERE utc_day<((clock_timestamp() AT TIME ZONE 'UTC')::date-DATE '1970-01-01')::bigint-30
      ORDER BY utc_day,user_id FOR UPDATE SKIP LOCKED LIMIT 1000
    ) DELETE FROM surprise_daily_decisions WHERE (user_id,utc_day) IN
      (SELECT user_id,utc_day FROM old_rows);
    GET DIAGNOSTICS other_removed=ROW_COUNT;
    RETURN removed+other_removed;
END $$;
