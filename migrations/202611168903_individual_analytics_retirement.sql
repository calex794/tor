-- I / REC-FLOW v2 P2b1. Atomic database cutover. The runtime/UI cutover is P2a;
-- an old process must still be stopped to erase its private RIL RAM/queue.
-- Outbox-first drains old canonical writers/consumers before the remaining
-- locks: no partially installed barrier can observe their later raw writes.
LOCK TABLE outbox_events IN ACCESS EXCLUSIVE MODE;
LOCK TABLE rec_subjects,rec_assignments,rec_attribution_events,rec_first_conversions,
    rec_reader_signals,organic_discovery_touches,organic_discovery_first_conversions,
    organic_discovery_unattributed_reads,flow_visits,flow_book_cohorts,
    reader_flow_read_receipts IN SHARE ROW EXCLUSIVE MODE;

-- Keep the approved C ports row-lockable and their column types unchanged.
CREATE OR REPLACE VIEW reader_activity_consent WITH (security_barrier=true) AS
SELECT user_id,subject_id,enabled_at FROM rec_subjects WHERE enabled AND false;
CREATE OR REPLACE VIEW reader_flow_consent WITH (security_barrier=true) AS
SELECT s.user_id,s.subject_id,s.flow_epoch,s.flow_enabled_at FROM rec_subjects s
WHERE s.enabled AND s.flow_epoch IS NOT NULL AND false
  AND EXISTS(SELECT 1 FROM users u WHERE u.id=s.user_id AND u.account_state='active');
CREATE OR REPLACE VIEW reader_flow_verified_book_actions WITH (security_barrier=true) AS
SELECT event_id,subject_id,flow_epoch,book_id,kind,occurred_at
FROM reader_flow_book_actions WHERE false;
CREATE OR REPLACE VIEW reader_flow_verified_activity_days WITH (security_barrier=true) AS
SELECT subject_id,flow_epoch,book_id,activity_day,first_seen_at
FROM reader_flow_activity_days WHERE false;
CREATE OR REPLACE VIEW reader_flow_verified_read_receipts WITH (security_barrier=true) AS
SELECT event_id,subject_id,flow_epoch,occurred_at FROM reader_flow_read_receipts WHERE false;
CREATE OR REPLACE FUNCTION reader_flow_capture_book_action() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$ BEGIN RETURN NULL; END $$;
CREATE OR REPLACE FUNCTION reader_flow_capture_read() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$ BEGIN RETURN NULL; END $$;

-- No new collection interval is invented: close at the last witnessed beat.
UPDATE reader_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL;
UPDATE reader_book_return_coverage SET ended_at=last_heartbeat_at WHERE ended_at IS NULL;

-- Even zero-row writes/old upserts fail; no operator flag or old consent can
-- revive these stores. Explicit P1 preferences and canonical C are absent.
DO $$
DECLARE relation_name text;
BEGIN
    FOREACH relation_name IN ARRAY ARRAY[
        'ril_behavior_consents','rec_subjects','rec_assignments','rec_attribution_events',
        'rec_first_conversions','rec_reader_signals','organic_discovery_touches',
        'organic_discovery_first_conversions','organic_discovery_unattributed_reads',
        'rec_dirty_buckets','organic_discovery_dirty','rec_quality_daily',
        'organic_discovery_quality_daily','rec_worker_health','flow_visits','flow_visit_receipts',
        'flow_visit_account_days','flow_visit_continuations','flow_book_cohorts',
        'flow_book_return_anchors','flow_book_action_receipts','flow_book_worker_health',
        'reader_activity_days','reader_return_subjects','reader_book_return_subjects',
        'reader_flow_book_actions','reader_flow_activity_days','reader_flow_read_receipts',
        'reader_return_daily','reader_book_return_daily','reader_return_coverage',
        'reader_book_return_coverage'
    ] LOOP
        EXECUTE format('CREATE TRIGGER retired_collection_write_fence BEFORE INSERT OR UPDATE ON %I
            FOR EACH STATEMENT EXECUTE FUNCTION rec_reject_retired_collection()',relation_name);
    END LOOP;
    FOREACH relation_name IN ARRAY ARRAY['flow_visit_daily','flow_book_daily'] LOOP
        EXECUTE format('CREATE TRIGGER retired_collection_insert_fence BEFORE INSERT ON %I
            FOR EACH STATEMENT EXECUTE FUNCTION rec_reject_retired_collection()',relation_name);
        EXECUTE format('CREATE TRIGGER retired_collection_retraction_guard BEFORE UPDATE ON %I
            FOR EACH ROW EXECUTE FUNCTION flow_allow_only_aggregate_retraction()',relation_name);
    END LOOP;
END
$$;

CREATE TRIGGER flow_retire_deleted_visits AFTER DELETE ON flow_visits
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION flow_retire_deleted_visits();
CREATE TRIGGER flow_retire_deleted_book_cohorts AFTER DELETE ON flow_book_cohorts
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION flow_retire_deleted_book_cohorts();
CREATE TRIGGER rec_retire_deleted_assignments AFTER DELETE ON rec_assignments
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION rec_retire_deleted_assignments();
CREATE TRIGGER rec_retire_deleted_organic_touches AFTER DELETE ON organic_discovery_touches
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION rec_retire_deleted_organic_touches();
CREATE TRIGGER rec_retire_deleted_unattributed_reads AFTER DELETE ON organic_discovery_unattributed_reads
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION rec_retire_deleted_unattributed_reads();
CREATE TRIGGER rec_retire_deleted_dirty_keys AFTER DELETE ON rec_dirty_buckets
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION rec_retire_deleted_dirty_keys();
CREATE TRIGGER rec_retire_deleted_organic_dirty_keys AFTER DELETE ON organic_discovery_dirty
    REFERENCING OLD TABLE AS retired_rows FOR EACH STATEMENT
    EXECUTE FUNCTION rec_retire_deleted_dirty_keys();

-- Retraction now belongs to the exact DELETE statement. The old explicit
-- erasure call and its scope trigger must not subtract the same input twice.
CREATE OR REPLACE FUNCTION flow_erase_scope(p_subject uuid,p_epoch uuid) RETURNS void
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(20261116,8903);
    PERFORM public.flow_book_erase_scope(p_subject,p_epoch);
    DELETE FROM flow_visits WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_visit_receipts WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_visit_account_days WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_visit_continuations WHERE subject_id=p_subject AND flow_epoch=p_epoch;
END
$$;
CREATE OR REPLACE FUNCTION flow_book_erase_scope(p_subject uuid,p_epoch uuid) RETURNS void
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(20261116,8903);
    DELETE FROM flow_book_cohorts WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM flow_book_action_receipts WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM reader_flow_book_actions WHERE subject_id=p_subject AND flow_epoch=p_epoch;
    DELETE FROM reader_flow_activity_days WHERE subject_id=p_subject AND flow_epoch=p_epoch;
END
$$;

-- Existing account and outbox parents can otherwise hold a row before a
-- child eraser reaches an advisory gate. Gate only DELETE, never ordinary
-- canonical inserts/updates, P1 commands or another consumer's ACK.
DO $$
DECLARE relation_name text;
BEGIN
    FOREACH relation_name IN ARRAY ARRAY[
        'users','books','outbox_events','rec_subjects','ril_behavior_consents',
        'rec_assignments','rec_attribution_events','rec_first_conversions','rec_reader_signals',
        'organic_discovery_touches','organic_discovery_first_conversions',
        'organic_discovery_unattributed_reads','rec_dirty_buckets','organic_discovery_dirty',
        'rec_quality_daily','organic_discovery_quality_daily','rec_analytics_invalidations',
        'flow_visits','flow_visit_receipts','flow_visit_account_days','flow_visit_continuations',
        'flow_book_cohorts','flow_book_return_anchors','flow_book_action_receipts',
        'flow_visit_daily','flow_book_daily','reader_flow_book_actions','reader_flow_activity_days',
        'reader_flow_read_receipts','reader_activity_days','reader_return_subjects',
        'reader_book_return_subjects','reader_return_daily','reader_book_return_daily',
        'reader_return_coverage','reader_book_return_coverage','event_deliveries','ril_flush_transitions'
    ] LOOP
        EXECUTE format('CREATE TRIGGER retired_erasure_gate BEFORE DELETE ON %I
            FOR EACH STATEMENT EXECUTE FUNCTION rec_lock_retired_erasure()',relation_name);
    END LOOP;
END
$$;

-- Retained hooks also run from account cascades outside the new batch.
ALTER FUNCTION flow_erase_replaced_scope() SET search_path=pg_catalog,public,pg_temp;
ALTER FUNCTION flow_book_erase_replaced_scope() SET search_path=pg_catalog,public,pg_temp;
ALTER FUNCTION reader_flow_erase_read_scope() SET search_path=pg_catalog,public,pg_temp;
ALTER FUNCTION reader_flow_erase_read_delivery() SET search_path=pg_catalog,public,pg_temp;
ALTER FUNCTION reader_return_erase_rotated_consent() SET search_path=pg_catalog,public,pg_temp;
ALTER FUNCTION reader_book_return_erase_rotated_consent() SET search_path=pg_catalog,public,pg_temp;

-- Empty obligations mean no consumer is required, not a fabricated receipt.
-- Preserve every canonical event and every non-retired consumer obligation.
ALTER TABLE outbox_events DROP CONSTRAINT outbox_events_required_consumers_check;
CREATE TRIGGER retired_outbox_obligations BEFORE INSERT OR UPDATE ON outbox_events
    FOR EACH ROW EXECUTE FUNCTION rec_normalize_retired_outbox_obligations();
CREATE TRIGGER retired_analytics_delivery_fence BEFORE INSERT OR UPDATE ON event_deliveries
    FOR EACH ROW EXECUTE FUNCTION rec_reject_retired_delivery();

-- Old producers may have mixed RIL batches already queued. Drop only the
-- retired components; let the existing batch receipt/lease protect numeric
-- route, SQL cost and anonymous HTML counters from replay as before.
CREATE TRIGGER ril_retire_individual_flow_counts BEFORE INSERT OR UPDATE ON ril_flush_flow
    FOR EACH ROW EXECUTE FUNCTION ril_retire_individual_flow_counts();
CREATE TRIGGER ril_freeze_individual_flow_counts BEFORE INSERT OR UPDATE ON ril_flow_hourly
    FOR EACH ROW EXECUTE FUNCTION ril_retire_individual_flow_counts();
CREATE TRIGGER ril_skip_retired_transition BEFORE INSERT OR UPDATE ON ril_flush_transitions
    FOR EACH ROW EXECUTE FUNCTION ril_skip_retired_transition();
CREATE TRIGGER ril_freeze_retired_transition BEFORE INSERT OR UPDATE ON ril_transition_hourly
    FOR EACH ROW EXECUTE FUNCTION ril_skip_retired_transition();

-- One transaction, one deletion budget across every table, including leaves.
-- No externally supplied SQL, no elevated privileges and no collection path.
-- Once barriers exist, leaf-first SKIP LOCKED cannot wait raw -> subject.
-- Parent predicates cover every FK child; account/book business cascades use
-- the same DELETE hooks but are separate, explicitly requested operations.
CREATE FUNCTION rec_retire_analytics_batch(p_limit integer DEFAULT 256)
RETURNS TABLE(erased_rows bigint,pending boolean)
LANGUAGE plpgsql
SET search_path=pg_catalog,public,pg_temp
SET lock_timeout='250ms'
AS $$
DECLARE item record; remaining integer:=p_limit; changed integer; still_present boolean;
BEGIN
    IF p_limit IS NULL OR p_limit<1 OR p_limit>1000 THEN
        RAISE EXCEPTION 'Retirement batch limit must be between 1 and 1000' USING ERRCODE='22023';
    END IF;
    erased_rows:=0;
    pending:=false;
    IF NOT pg_try_advisory_xact_lock(20261116,8903) THEN
        pending:=true;
        RETURN NEXT;
        RETURN;
    END IF;
    FOR item IN SELECT * FROM (VALUES
        ('rec_dirty_buckets','true','t.bucket_date,t.book_id','delete'),
        ('organic_discovery_dirty','true','t.bucket_date,t.book_id','delete'),
        ('rec_attribution_events','true','t.id','delete'),
        ('rec_first_conversions','true','t.subject_id,t.book_id,t.kind','delete'),
        ('organic_discovery_first_conversions','true','t.subject_id,t.book_id,t.kind','delete'),
        ('flow_book_return_anchors','true','t.subject_id,t.flow_epoch,t.book_id','delete'),
        ('event_deliveries',
            't.consumer IN (''recommendation-feedback-v1'',''flow-read-attribution-v1'')',
            't.consumer,t.event_id','delete'),
        ('rec_assignments',
            'NOT EXISTS(SELECT 1 FROM public.rec_attribution_events c WHERE c.assignment_id=t.id)
             AND NOT EXISTS(SELECT 1 FROM public.rec_first_conversions c WHERE c.assignment_id=t.id)',
            't.id','delete'),
        ('organic_discovery_touches',
            'NOT EXISTS(SELECT 1 FROM public.organic_discovery_first_conversions c WHERE c.touch_id=t.id)',
            't.id','delete'),
        ('organic_discovery_unattributed_reads','true','t.subject_id,t.book_id,t.kind','delete'),
        ('rec_reader_signals','true','t.subject_id,t.book_id','delete'),
        ('flow_book_cohorts',
            'NOT EXISTS(SELECT 1 FROM public.flow_book_return_anchors c WHERE c.cohort_id=t.id)',
            't.id','delete'),
        ('flow_visits','true','t.id','delete'),
        ('flow_visit_receipts','true','t.event_id','delete'),
        ('flow_visit_account_days','true','t.subject_id,t.flow_epoch,t.bucket_date','delete'),
        ('flow_visit_continuations','true','t.subject_id,t.flow_epoch','delete'),
        ('flow_book_action_receipts','true','t.event_id','delete'),
        ('reader_flow_book_actions','true','t.event_id','delete'),
        ('reader_flow_activity_days','true','t.subject_id,t.flow_epoch,t.book_id,t.activity_day','delete'),
        ('reader_flow_read_receipts',
            'NOT EXISTS(SELECT 1 FROM public.event_deliveries d
                WHERE d.event_id=t.event_id AND d.consumer=''flow-read-attribution-v1'')',
            't.event_id','delete'),
        ('reader_activity_days','true','t.subject_id,t.book_id,t.activity_day','delete'),
        ('reader_return_subjects','true','t.subject_id','delete'),
        ('reader_book_return_subjects','true','t.subject_id,t.book_id,t.cohort_day','delete'),
        ('ril_behavior_consents','true','t.user_id','delete'),
        ('ril_flush_transitions','true','t.batch_id,t.from_code,t.to_code,t.gap_group','delete'),
        ('ril_flush_flow','t.opted_in_html_requests>0 OR t.observed_opted_in_visit_starts>0',
            't.batch_id,t.surface_code,t.acquisition_hint','scrub'),
        ('rec_subjects',
            'NOT EXISTS(SELECT 1 FROM public.rec_feedback_state c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.rec_assignments c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.rec_first_conversions c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.rec_reader_signals c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_touches c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_first_conversions c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_unattributed_reads c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.flow_visits c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.flow_visit_receipts c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.flow_visit_account_days c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.flow_visit_continuations c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.flow_book_cohorts c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.flow_book_action_receipts c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.reader_flow_book_actions c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.reader_flow_activity_days c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.reader_flow_read_receipts c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.reader_activity_days c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.reader_return_subjects c WHERE c.subject_id=t.subject_id)
             AND NOT EXISTS(SELECT 1 FROM public.reader_book_return_subjects c WHERE c.subject_id=t.subject_id)',
            't.subject_id','delete'),
        ('outbox_events',
            't.required_consumers && ARRAY[''recommendation-feedback-v1'',''flow-read-attribution-v1'']
             OR (cardinality(t.required_consumers)=0 AND t.delivered_at IS NULL)',
            't.id','normalize'),
        ('rec_quality_daily',
            't.bucket_date<(clock_timestamp() AT TIME ZONE ''UTC'')::date-180
             OR EXISTS(SELECT 1 FROM public.rec_analytics_invalidations i
                WHERE i.source=''recommendations'' AND i.bucket_date=t.bucket_date AND i.book_id=t.book_id)',
            't.bucket_date,t.book_id,t.surface,t.source,t.reason,t.position,t.generation,t.algorithm_version','delete'),
        ('organic_discovery_quality_daily',
            't.bucket_date<(clock_timestamp() AT TIME ZONE ''UTC'')::date-180
             OR EXISTS(SELECT 1 FROM public.rec_analytics_invalidations i
                WHERE i.source=t.source AND i.bucket_date=t.bucket_date AND i.book_id=t.book_id)',
            't.bucket_date,t.book_id,t.source','delete'),
        ('flow_visit_daily','t.cohort_day<(clock_timestamp() AT TIME ZONE ''UTC'')::date-179',
            't.cohort_day,t.acquisition_hint,t.landing_surface,t.last_surface','delete'),
        ('flow_book_daily','t.cohort_day<(clock_timestamp() AT TIME ZONE ''UTC'')::date-179',
            't.cohort_day,t.book_id,t.acquisition_hint,t.discovery_kind','delete'),
        ('reader_return_daily','t.cohort_day<(clock_timestamp() AT TIME ZONE ''UTC'')::date-180',
            't.cohort_day,t.day','delete'),
        ('reader_book_return_daily','t.cohort_day<=(clock_timestamp() AT TIME ZONE ''UTC'')::date-180',
            't.book_id,t.cohort_day,t.day','delete'),
        ('reader_return_coverage',
            't.ended_at<clock_timestamp()-interval ''180 days''
             AND t.id<>(SELECT min(id) FROM public.reader_return_coverage)
             AND NOT EXISTS(SELECT 1 FROM public.reader_return_subjects s WHERE s.coverage_epoch_id=t.id)',
            't.id','delete'),
        ('reader_book_return_coverage',
            't.ended_at<clock_timestamp()-interval ''180 days''
             AND t.id<>(SELECT min(id) FROM public.reader_book_return_coverage)',
            't.id','delete'),
        ('rec_analytics_invalidations',
            't.bucket_date<(clock_timestamp() AT TIME ZONE ''UTC'')::date-180
             AND NOT EXISTS(SELECT 1 FROM public.rec_quality_daily d
                WHERE t.source=''recommendations'' AND d.bucket_date=t.bucket_date AND d.book_id=t.book_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_quality_daily d
                WHERE d.source=t.source AND d.bucket_date=t.bucket_date AND d.book_id=t.book_id)
             AND NOT EXISTS(SELECT 1 FROM public.rec_assignments r
                WHERE t.source=''recommendations'' AND r.cohort_day=t.bucket_date AND r.book_id=t.book_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_touches r
                WHERE r.source=t.source AND r.cohort_day=t.bucket_date AND r.book_id=t.book_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_unattributed_reads r
                WHERE t.source=''unattributed'' AND r.bucket_date=t.bucket_date AND r.book_id=t.book_id)
             AND NOT EXISTS(SELECT 1 FROM public.rec_dirty_buckets q
                WHERE t.source=''recommendations'' AND q.bucket_date=t.bucket_date AND q.book_id=t.book_id)
             AND NOT EXISTS(SELECT 1 FROM public.organic_discovery_dirty q
                WHERE t.source<>''recommendations'' AND q.bucket_date=t.bucket_date AND q.book_id=t.book_id)',
            't.source,t.bucket_date,t.book_id','delete')
    ) AS work(relation_name,predicate,ordering,operation)
    LOOP
        IF remaining>0 THEN
            IF item.operation='delete' THEN
                EXECUTE format('WITH due AS MATERIALIZED (
                    SELECT t.ctid FROM public.%I t WHERE %s ORDER BY %s
                    LIMIT $1 FOR UPDATE OF t SKIP LOCKED)
                    DELETE FROM public.%I t USING due WHERE t.ctid=due.ctid',
                    item.relation_name,item.predicate,item.ordering,item.relation_name)
                    USING remaining;
            ELSIF item.operation='scrub' THEN
                EXECUTE format('WITH due AS MATERIALIZED (
                    SELECT t.ctid FROM public.%I t WHERE %s ORDER BY %s
                    LIMIT $1 FOR UPDATE OF t SKIP LOCKED)
                    UPDATE public.%I t SET opted_in_html_requests=0,observed_opted_in_visit_starts=0
                    FROM due WHERE t.ctid=due.ctid',
                    item.relation_name,item.predicate,item.ordering,item.relation_name)
                    USING remaining;
            ELSE
                EXECUTE format('WITH due AS MATERIALIZED (
                    SELECT t.ctid FROM public.%I t WHERE %s ORDER BY %s
                    LIMIT $1 FOR UPDATE OF t SKIP LOCKED)
                    UPDATE public.%I t SET required_consumers=t.required_consumers
                    FROM due WHERE t.ctid=due.ctid',
                    item.relation_name,item.predicate,item.ordering,item.relation_name)
                    USING remaining;
            END IF;
            GET DIAGNOSTICS changed=ROW_COUNT;
            remaining:=remaining-changed;
            IF item.operation='delete' THEN erased_rows:=erased_rows+changed; END IF;
        END IF;
        EXECUTE format('SELECT EXISTS(SELECT 1 FROM public.%I t WHERE %s)',
            item.relation_name,item.predicate) INTO still_present;
        pending:=pending OR still_present;
    END LOOP;
    RETURN NEXT;
END
$$;
COMMENT ON FUNCTION rec_retire_analytics_batch(integer) IS
    'P2b1 bounded retirement: at most limit deleted/normalized/scrubbed rows, FK leaves first, <=3*limit invalidation keys; canonical C/outbox payloads/P1 untouched. pending is conservative under concurrent workers; not a proof that old process RAM has stopped.';
