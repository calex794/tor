-- I / REC-FLOW v2 P2b1. Preparatory definitions only: the next migration
-- installs every barrier and erasure hook in one transaction. No old writer
-- changes behavior between these two migrations.
CREATE TABLE rec_analytics_invalidations (
    source text NOT NULL CHECK(source IN ('recommendations','catalog','search','unattributed')),
    bucket_date date NOT NULL,
    book_id bigint NOT NULL CHECK(book_id>0),
    PRIMARY KEY(source,bucket_date,book_id)
);
CREATE INDEX rec_analytics_invalidations_book
    ON rec_analytics_invalidations(book_id,bucket_date,source);
CREATE INDEX rec_analytics_invalidations_retention
    ON rec_analytics_invalidations(bucket_date,source,book_id);
COMMENT ON TABLE rec_analytics_invalidations IS
    'Anonymous evidence absence, not a zero counter: exact source/UTC day/book only; no subject/event IDs and no book FK; same 180-day boundary as legacy REC/organic daily.';

CREATE FUNCTION rec_reject_retired_collection() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    RAISE EXCEPTION 'Individual analytics collection has been retired'
        USING ERRCODE='55000';
END
$$;

-- Aggregates already beyond their existing retention horizon need no
-- retraction. A missing still-retained FLOW bucket is an integrity failure.
-- An entire book may be deleted canonically; its book aggregate cascades too.
CREATE FUNCTION flow_retire_deleted_visits() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
DECLARE b record;
BEGIN
    FOR b IN
        SELECT (started_at AT TIME ZONE 'UTC')::date AS day,
               acquisition_hint,landing_surface,last_surface,
               count(*) AS visits,sum(pageviews)::bigint AS pageviews,
               sum(floor(extract(epoch FROM(last_at-started_at)))::bigint)::bigint AS seconds
        FROM retired_rows
        WHERE finalized_at IS NOT NULL
          AND (started_at AT TIME ZONE 'UTC')::date>=(clock_timestamp() AT TIME ZONE 'UTC')::date-179
        GROUP BY 1,2,3,4 ORDER BY 1,2,3,4
    LOOP
        UPDATE flow_visit_daily d SET visits=d.visits-b.visits,
            pageviews=d.pageviews-b.pageviews,observed_seconds=d.observed_seconds-b.seconds
        WHERE d.cohort_day=b.day AND d.acquisition_hint=b.acquisition_hint
          AND d.landing_surface=b.landing_surface AND d.last_surface=b.last_surface;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'Retained FLOW visit aggregate is unavailable' USING ERRCODE='23514';
        END IF;
    END LOOP;
    RETURN NULL;
END
$$;

CREATE FUNCTION flow_retire_deleted_book_cohorts() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
DECLARE b record;
BEGIN
    FOR b IN
        SELECT (first_entry_at AT TIME ZONE 'UTC')::date AS day,
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
        FROM retired_rows
        WHERE finalized_at IS NOT NULL
          AND (first_entry_at AT TIME ZONE 'UTC')::date>=(clock_timestamp() AT TIME ZONE 'UTC')::date-179
        GROUP BY 1,2,3,4 ORDER BY 1,2,3,4
    LOOP
        UPDATE flow_book_daily d SET cohorts=d.cohorts-b.cohorts,
            detail_opens=d.detail_opens-b.detail_opens,reader_opens=d.reader_opens-b.reader_opens,
            starts=d.starts-b.starts,completions=d.completions-b.completions,
            saves=d.saves-b.saves,follows=d.follows-b.follows,new_readers=d.new_readers-b.new_readers,
            d7_anchors=d.d7_anchors-b.d7_anchors,d7_returned=d.d7_returned-b.d7_returned,
            d7_unknown=d.d7_unknown-b.d7_unknown
        WHERE d.cohort_day=b.day AND d.book_id=b.book_id
          AND d.acquisition_hint=b.acquisition_hint AND d.discovery_kind=b.discovery_kind;
        IF NOT FOUND AND EXISTS(SELECT 1 FROM books WHERE id=b.book_id) THEN
            RAISE EXCEPTION 'Retained FLOW book aggregate is unavailable' USING ERRCODE='23514';
        END IF;
    END LOOP;
    RETURN NULL;
END
$$;

-- The legacy REC/organic materializer has no per-input materialization proof.
-- Do not subtract guessed counts or rebuild a partial day as a complete zero.
-- Transition tables also cover old TTL deletes and canonical FK cascades.
CREATE FUNCTION rec_retire_deleted_assignments() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
    SELECT DISTINCT 'recommendations',cohort_day,book_id FROM retired_rows
    WHERE cohort_day>=(clock_timestamp() AT TIME ZONE 'UTC')::date-180
    ORDER BY 1,2,3 ON CONFLICT DO NOTHING;
    RETURN NULL;
END
$$;

CREATE FUNCTION rec_retire_deleted_organic_touches() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
    SELECT DISTINCT source,cohort_day,book_id FROM retired_rows
    WHERE cohort_day>=(clock_timestamp() AT TIME ZONE 'UTC')::date-180
    ORDER BY 1,2,3 ON CONFLICT DO NOTHING;
    RETURN NULL;
END
$$;

CREATE FUNCTION rec_retire_deleted_unattributed_reads() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
    SELECT DISTINCT 'unattributed',bucket_date,book_id FROM retired_rows
    WHERE bucket_date>=(clock_timestamp() AT TIME ZONE 'UTC')::date-180
    ORDER BY 1,2,3 ON CONFLICT DO NOTHING;
    RETURN NULL;
END
$$;

-- A queued old recomputation may already have lost its last raw input. Its
-- missing channel cannot be guessed: the organic queue has only day/book.
-- Counters become unavailable as soon as the marker commits. Their physical
-- removal is a separate bounded operation, not an unbounded trigger fan-out.
CREATE FUNCTION rec_retire_deleted_dirty_keys() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    INSERT INTO rec_analytics_invalidations(source,bucket_date,book_id)
    SELECT DISTINCT channel,r.bucket_date,r.book_id FROM retired_rows r
    CROSS JOIN unnest(CASE TG_TABLE_NAME WHEN 'rec_dirty_buckets' THEN
        ARRAY['recommendations'] ELSE ARRAY['catalog','search','unattributed'] END) channel
    WHERE r.bucket_date>=(clock_timestamp() AT TIME ZONE 'UTC')::date-180
    ORDER BY 1,2,3 ON CONFLICT DO NOTHING;
    RETURN NULL;
END
$$;

-- Only erasure can update frozen FLOW totals. Existing CHECKs still enforce
-- nonnegative counters and funnel relationships; no key may be moved.
CREATE FUNCTION flow_allow_only_aggregate_retraction() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
DECLARE old_row jsonb:=to_jsonb(OLD); new_row jsonb:=to_jsonb(NEW); fields text[]; field text;
BEGIN
    fields:=CASE TG_TABLE_NAME WHEN 'flow_visit_daily' THEN
        ARRAY['visits','pageviews','observed_seconds'] ELSE
        ARRAY['cohorts','detail_opens','reader_opens','starts','completions','saves','follows',
              'new_readers','d7_anchors','d7_returned','d7_unknown'] END;
    IF old_row-fields IS DISTINCT FROM new_row-fields THEN
        RAISE EXCEPTION 'Retired FLOW aggregate key is immutable' USING ERRCODE='55000';
    END IF;
    FOREACH field IN ARRAY fields LOOP
        IF (new_row->>field)::bigint>(old_row->>field)::bigint THEN
            RAISE EXCEPTION 'Retired FLOW aggregate cannot grow' USING ERRCODE='55000';
        END IF;
    END LOOP;
    RETURN NEW;
END
$$;

CREATE FUNCTION rec_normalize_retired_outbox_obligations() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    NEW.required_consumers:=array_remove(array_remove(NEW.required_consumers,
        'recommendation-feedback-v1'),'flow-read-attribution-v1');
    IF cardinality(NEW.required_consumers)=0 OR NOT EXISTS(
        SELECT 1 FROM unnest(NEW.required_consumers) AS c(consumer)
        WHERE NOT EXISTS(SELECT 1 FROM event_deliveries d
            WHERE d.event_id=NEW.id AND d.consumer=c.consumer)) THEN
        NEW.delivered_at:=COALESCE(NEW.delivered_at,clock_timestamp());
        NEW.lease_until:=NULL;
    END IF;
    RETURN NEW;
END
$$;
CREATE FUNCTION rec_reject_retired_delivery() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    IF NEW.consumer IN ('recommendation-feedback-v1','flow-read-attribution-v1')
        OR (TG_OP='UPDATE' AND OLD.consumer IN ('recommendation-feedback-v1','flow-read-attribution-v1')) THEN
        RAISE EXCEPTION 'Individual analytics consumer has been retired' USING ERRCODE='55000';
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION ril_retire_individual_flow_counts() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    IF TG_TABLE_NAME='ril_flow_hourly' AND TG_OP='UPDATE' THEN
        NEW.opted_in_html_requests:=OLD.opted_in_html_requests;
        NEW.observed_opted_in_visit_starts:=OLD.observed_opted_in_visit_starts;
    ELSE
        NEW.opted_in_html_requests:=0;
        NEW.observed_opted_in_visit_starts:=0;
    END IF;
    RETURN NEW;
END
$$;
CREATE FUNCTION ril_skip_retired_transition() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    RETURN NULL;
END
$$;

-- A transaction-scoped gate serializes retirement erasers across FK table
-- order. It carries no actor/key history and creates no persistent ledger.
CREATE FUNCTION rec_lock_retired_erasure() RETURNS trigger
LANGUAGE plpgsql SET search_path=pg_catalog,public,pg_temp AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(20261116,8903);
    RETURN NULL;
END
$$;
