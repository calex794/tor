//! Synthetic installed-190 records, shared by upgrade and actual-binary proofs.
//! This module never disables a trigger or seeds retired data after the barrier.
use sqlx::PgPool;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub async fn run(pool: &PgPool, sql: &str) -> TestResult {
    sqlx::raw_sql(sql).execute(pool).await?;
    Ok(())
}

pub async fn text(pool: &PgPool, sql: &str) -> TestResult<String> {
    Ok(sqlx::query_scalar(sql).fetch_one(pool).await?)
}

pub async fn count(pool: &PgPool, sql: &str) -> TestResult<i64> {
    Ok(sqlx::query_scalar(sql).fetch_one(pool).await?)
}

async fn actor(pool: &PgPool, email: &str) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Installed analytics fixture','$argon2id$test') RETURNING id",
    )
    .bind(email)
    .fetch_one(pool)
    .await?)
}

async fn book(pool: &PgPool, source: &str) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Installed analytics book','public','basic') RETURNING id",
    )
    .bind(source)
    .fetch_one(pool)
    .await?)
}

pub struct Fixture {
    pub actor: i64,
    pub subject: String,
    pub epoch: String,
    pub book: i64,
    pub untouched_book: i64,
    pub dirty_book: i64,
    pub boundary_book: i64,
    pub chapter: i64,
    pub version: i64,
    pub event: String,
    pub delete_event: String,
    pub assignment: String,
    pub cascade_actor: i64,
    pub cascade_book: i64,
    pub ril_batch: String,
}

impl Fixture {
    pub async fn seed(pool: &PgPool) -> TestResult<Self> {
        let actor_id = actor(pool, "retirement-main@ci.invalid").await?;
        let cascade_actor = actor(pool, "retirement-cascade@ci.invalid").await?;
        let book_id = book(pool, "ci:retirement:linked").await?;
        let untouched_book = book(pool, "ci:retirement:untouched").await?;
        let dirty_book = book(pool, "ci:retirement:queued").await?;
        let boundary_book = book(pool, "ci:retirement:boundary").await?;
        let cascade_book = book(pool, "ci:retirement:cascade").await?;
        run(pool, &format!(
            "INSERT INTO rec_subjects(user_id,enabled,revision,enabled_at,flow_epoch,flow_enabled_at)
             SELECT u,true,9,now()-interval '20 days',gen_random_uuid(),now()-interval '20 days'
             FROM unnest(ARRAY[{actor_id},{cascade_actor}]::bigint[]) u"
        )).await?;
        let (subject, epoch): (String, String) = sqlx::query_as(
            "SELECT subject_id::text,flow_epoch::text FROM rec_subjects WHERE user_id=$1",
        )
        .bind(actor_id)
        .fetch_one(pool)
        .await?;
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
             VALUES($1,1,'Installed canonical chapter','public','free') RETURNING id",
        )
        .bind(book_id)
        .fetch_one(pool)
        .await?;
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,'Canonical content remains unchanged') RETURNING id",
        )
        .bind(chapter)
        .fetch_one(pool)
        .await?;
        run(
            pool,
            &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
        )
        .await?;
        let event = text(pool, &format!(
            "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,required_consumers,created_at,lease_until)
             VALUES('ChapterStartedV1',1,'chapter',{chapter},'ci:retirement:canonical',
                jsonb_build_object('user_id','{actor_id}','book_id','{book_id}',
                  'chapter_id','{chapter}','version_id','{version}','authority','C-verified-free-start'),
                ARRAY['recommendation-feedback-v1','reading-rankings-v1'],
                now()-interval '2 days',now()+interval '10 minutes')
             RETURNING id::text"
        )).await?;
        // No canonical business row points to this separate parent-delete probe.
        let delete_event = text(
            pool,
            "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,required_consumers)
             VALUES('RetirementProbeV1',1,'test',1,'ci:retirement:delete-probe','{}',
                ARRAY['recommendation-feedback-v1']) RETURNING id::text",
        )
        .await?;
        run(
            pool,
            &format!(
                "INSERT INTO reader_chapter_starts(user_id,book_id,chapter_id,first_version_id,
                started_at,source_event_id)
             VALUES({actor_id},{book_id},{chapter},{version},now()-interval '2 days','{event}');
             INSERT INTO reader_library_items(user_id,book_id) VALUES({actor_id},{book_id});
             INSERT INTO reader_book_follows(user_id,book_id) VALUES({actor_id},{book_id});
             INSERT INTO reader_flow_read_receipts(event_id,subject_id,flow_epoch,occurred_at)
             VALUES('{event}','{subject}','{epoch}',now()-interval '2 days'),
                   ('{delete_event}','{subject}','{epoch}',now());
             INSERT INTO event_deliveries(consumer,event_id)
             VALUES('recommendation-feedback-v1','{event}'),
                   ('flow-read-attribution-v1','{event}'),('unrelated-v1','{event}');
             INSERT INTO rec_preference_accounts(user_id,feedback_epoch) VALUES({actor_id},4);
             INSERT INTO rec_book_preferences(user_id,book_id,hidden,revision,expires_at)
             VALUES({actor_id},{book_id},true,7,now()+interval '4 days');
             INSERT INTO rec_user_preference_commands(user_id,command_key,payload_hash,
                result_revision,command_type,created_at)
             VALUES({actor_id},decode(repeat('11',32),'hex'),decode(repeat('aa',32),'hex'),
                    4000000007,'feedback',now()-interval '2 days'),
                   ({actor_id},decode(repeat('22',32),'hex'),decode(repeat('bb',32),'hex'),
                    9,'consent',now()-interval '2 days')"
            ),
        )
        .await?;
        let assignment = text(
            pool,
            &format!(
                "INSERT INTO rec_assignments(subject_id,session_digest,ticket_digest,book_id,
                surface,source,reason,position,created_at,expires_at,viewed_at,cohort_day)
             VALUES('{subject}',decode(repeat('11',32),'hex'),decode(repeat('22',32),'hex'),
                {book_id},'home','content','similar_by_content',1,now()-interval '2 days',
                now()-interval '1 day',now()-interval '2 days',
                (now() AT TIME ZONE 'UTC')::date-2) RETURNING id::text"
            ),
        )
        .await?;
        run(pool, &format!(
            "INSERT INTO rec_attribution_events(assignment_id,kind,source_event_id,occurred_at,trust)
             SELECT '{assignment}'::uuid,kind,
                CASE WHEN trust='domain_committed' THEN gen_random_uuid() END,now(),trust
             FROM (VALUES('impression','client_observed'),('click','server_navigation'),
                ('book_opened','server_navigation'),('chapter_started','domain_committed'),
                ('chapter_finished','domain_committed')) AS e(kind,trust);
             INSERT INTO rec_first_conversions(subject_id,book_id,kind,assignment_id,source_event_id,occurred_at)
             SELECT '{subject}'::uuid,{book_id},kind,'{assignment}'::uuid,gen_random_uuid(),now()
             FROM unnest(ARRAY['chapter_started','chapter_finished']) kind;
             INSERT INTO rec_reader_signals(subject_id,book_id,occurred_at,source_event_id)
             VALUES('{subject}',{book_id},now()-interval '1 day',gen_random_uuid());
             INSERT INTO organic_discovery_touches(subject_id,session_digest,ticket_digest,
                book_id,source,position,created_at,expires_at,cohort_day)
             VALUES('{subject}',decode(repeat('33',32),'hex'),decode(repeat('44',32),'hex'),
                {book_id},'catalog',1,now()-interval '2 days',now()-interval '1 day',
                (now() AT TIME ZONE 'UTC')::date-2);
             INSERT INTO organic_discovery_first_conversions(subject_id,book_id,kind,touch_id,
                source_event_id,occurred_at)
             SELECT subject_id,book_id,'chapter_started',id,gen_random_uuid(),now()
             FROM organic_discovery_touches WHERE subject_id='{subject}';
             INSERT INTO organic_discovery_unattributed_reads(subject_id,book_id,kind,
                source_event_id,occurred_at,bucket_date)
             VALUES('{subject}',{book_id},'chapter_started',gen_random_uuid(),
                now()-interval '1 day',(now() AT TIME ZONE 'UTC')::date-1);
             INSERT INTO rec_dirty_buckets VALUES((now() AT TIME ZONE 'UTC')::date-5,{dirty_book});
             INSERT INTO organic_discovery_dirty VALUES((now() AT TIME ZONE 'UTC')::date-5,{dirty_book})"
        )).await?;
        for b in [book_id, untouched_book, dirty_book] {
            run(pool, &format!(
                "INSERT INTO rec_quality_daily(bucket_date,book_id,surface,source,reason,position,
                    generation,algorithm_version,impressions,paired_clicks,unpaired_clicks,
                    book_opens,starts,completions)
                 VALUES((now() AT TIME ZONE 'UTC')::date-2,{b},'home','content','similar_by_content',
                    1,'old-generation','recommendations_v1',13,4,1,7,5,2);
                 INSERT INTO organic_discovery_quality_daily(bucket_date,book_id,source,
                    issued,clicks,book_opens,starts,completions)
                 SELECT (now() AT TIME ZONE 'UTC')::date-2,{b},source,11,5,4,3,2
                 FROM unnest(ARRAY['catalog','search','unattributed']) source"
            )).await?;
        }
        // Exact 180-day edge; the expired raw and dirty key must not keep the
        // old counters alive, nor remove the still-retained boundary marker.
        run(pool, &format!(
            "INSERT INTO rec_quality_daily(bucket_date,book_id,surface,source,reason,position,
                generation,algorithm_version,impressions,paired_clicks,unpaired_clicks,book_opens,starts,completions)
             SELECT (now() AT TIME ZONE 'UTC')::date-age,{boundary_book},'home','content','boundary',1,
                'boundary','recommendations_v1',1,0,0,0,0,0 FROM unnest(ARRAY[180,181,182]) age;
             INSERT INTO rec_assignments(subject_id,session_digest,ticket_digest,book_id,surface,source,
                reason,position,created_at,expires_at,cohort_day)
             VALUES('{subject}',decode(repeat('77',32),'hex'),decode(repeat('88',32),'hex'),
                {boundary_book},'home','content','boundary',1,now()-interval '181 days',
                now()-interval '180 days',(now() AT TIME ZONE 'UTC')::date-181);
             INSERT INTO rec_dirty_buckets VALUES((now() AT TIME ZONE 'UTC')::date-182,{boundary_book});
             INSERT INTO rec_assignments(subject_id,session_digest,ticket_digest,book_id,surface,source,
                reason,position,created_at,expires_at,cohort_day)
             SELECT subject_id,decode(repeat('55',32),'hex'),decode(repeat('66',32),'hex'),
                {cascade_book},'home','content','cascade',1,now()-interval '2 days',
                now()-interval '1 day',(now() AT TIME ZONE 'UTC')::date-2
             FROM rec_subjects WHERE user_id={cascade_actor}"
        )).await?;
        for (a, b) in [(actor_id, book_id), (cascade_actor, cascade_book)] {
            seed_flow(pool, a, b).await?;
        }
        run(pool, &format!(
            "INSERT INTO reader_flow_book_actions(subject_id,flow_epoch,book_id,kind,occurred_at)
             VALUES('{subject}','{epoch}',{book_id},1,now()-interval '1 day');
             INSERT INTO reader_flow_activity_days(subject_id,flow_epoch,book_id,activity_day,first_seen_at)
             VALUES('{subject}','{epoch}',{book_id},(now() AT TIME ZONE 'UTC')::date,now());
             INSERT INTO reader_activity_days(subject_id,book_id,activity_day,first_seen_at)
             VALUES('{subject}',{book_id},(now() AT TIME ZONE 'UTC')::date,now());
             INSERT INTO reader_return_subjects(subject_id) VALUES('{subject}');
             INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day)
             VALUES('{subject}',{book_id},(now() AT TIME ZONE 'UTC')::date-2);
             INSERT INTO ril_behavior_consents(user_id,enabled,epoch) VALUES({actor_id},true,7);
             INSERT INTO reader_return_coverage(started_at,last_heartbeat_at)
             VALUES(now()-interval '20 days',now()-interval '10 minutes');
             INSERT INTO reader_book_return_coverage(started_at,last_heartbeat_at)
             VALUES(now()-interval '20 days',now()-interval '10 minutes')"
        )).await?;
        let ril_batch = text(pool, "SELECT gen_random_uuid()::text").await?;
        Ok(Self {
            actor: actor_id,
            subject,
            epoch,
            book: book_id,
            untouched_book,
            dirty_book,
            boundary_book,
            chapter,
            version,
            event,
            delete_event,
            assignment,
            cascade_actor,
            cascade_book,
            ril_batch,
        })
    }

    pub async fn canonical(&self, pool: &PgPool) -> TestResult<Vec<String>> {
        let mut result = Vec::new();
        for relation in [
            "reader_chapter_starts",
            "reader_library_items",
            "reader_book_follows",
        ] {
            result.push(text(pool, &format!(
                "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)::text
                 FROM {relation} t WHERE user_id={} AND book_id={}", self.actor, self.book
            )).await?);
        }
        result.push(
            text(
                pool,
                &format!(
            "SELECT jsonb_build_array(id,event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,created_at,attempts,next_attempt_at,lease_epoch,dead_at)::text
             FROM outbox_events WHERE id='{}'", self.event
        ),
            )
            .await?,
        );
        result.push(
            text(
                pool,
                &format!(
                    "SELECT to_jsonb(d)::text FROM event_deliveries d
             WHERE event_id='{}' AND consumer='unrelated-v1'",
                    self.event
                ),
            )
            .await?,
        );
        Ok(result)
    }

    pub async fn preferences(&self, pool: &PgPool) -> TestResult<Vec<String>> {
        let mut result = Vec::new();
        for relation in [
            "rec_preference_accounts",
            "rec_book_preferences",
            "rec_user_preference_commands",
        ] {
            result.push(text(pool, &format!(
                "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)::text
                 FROM {relation} t WHERE user_id={}", self.actor
            )).await?);
        }
        Ok(result)
    }
}

async fn seed_flow(pool: &PgPool, actor: i64, book: i64) -> TestResult {
    run(pool, &format!(
        "WITH seeded AS MATERIALIZED (
           SELECT s.subject_id,s.flow_epoch,gen_random_uuid() AS id,surface::smallint,
                  now()-interval '2 days' AS at
           FROM rec_subjects s CROSS JOIN unnest(ARRAY[1,2]) surface WHERE user_id={actor}
         ) INSERT INTO flow_visits(id,subject_id,flow_epoch,started_at,last_at,first_event_id,last_event_id,
             landing_surface,last_surface,acquisition_hint,pageviews,finalized_at)
           SELECT id,subject_id,flow_epoch,at,at+interval '10 minutes',id,id,surface,3,1,3,
                  now()-interval '1 day' FROM seeded;
         INSERT INTO flow_visits(subject_id,flow_epoch,started_at,last_at,first_event_id,last_event_id,
             landing_surface,last_surface,acquisition_hint,pageviews,finalized_at)
           SELECT subject_id,flow_epoch,now()-interval '181 days',now()-interval '181 days',
               gen_random_uuid(),gen_random_uuid(),4,4,3,1,now()
           FROM rec_subjects WHERE user_id={actor};
         INSERT INTO flow_visit_daily(cohort_day,acquisition_hint,landing_surface,last_surface,
             visits,pageviews,observed_seconds)
           SELECT (now() AT TIME ZONE 'UTC')::date-2,1,surface,3,2,6,1200
           FROM unnest(ARRAY[1,2]) surface
         ON CONFLICT(cohort_day,acquisition_hint,landing_surface,last_surface) DO UPDATE SET
           visits=flow_visit_daily.visits+2,pageviews=flow_visit_daily.pageviews+6,
           observed_seconds=flow_visit_daily.observed_seconds+1200;
         INSERT INTO flow_visit_receipts(event_id,subject_id,flow_epoch,observed_at,surface,acquisition_hint)
           SELECT id,subject_id,flow_epoch,started_at,landing_surface,acquisition_hint FROM flow_visits
           WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id={actor});
         INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
           SELECT subject_id,flow_epoch,(now() AT TIME ZONE 'UTC')::date
           FROM rec_subjects WHERE user_id={actor};
         INSERT INTO flow_visit_continuations(subject_id,flow_epoch,last_at)
           SELECT subject_id,flow_epoch,now() FROM rec_subjects WHERE user_id={actor};
         INSERT INTO flow_book_cohorts(id,subject_id,flow_epoch,visit_id,visit_started_at,book_id,
             first_entry_at,expires_at,acquisition_hint,discovery_kind,has_detail,has_reader,
             started,completed,first_start_at,first_start_event_id,first_completion_event_id,d7_state,finalized_at)
           SELECT gen_random_uuid(),subject_id,flow_epoch,gen_random_uuid(),now()-interval '2 days',
             {book},now()-interval '2 days',now()+interval '28 days',1,3,true,true,true,true,
             now()-interval '1 day',gen_random_uuid(),gen_random_uuid(),3,now()
           FROM rec_subjects WHERE user_id={actor};
         INSERT INTO flow_book_return_anchors(subject_id,flow_epoch,book_id,cohort_id,started_at,expires_at)
           SELECT subject_id,flow_epoch,book_id,id,first_start_at,expires_at
           FROM flow_book_cohorts WHERE book_id={book};
         INSERT INTO flow_book_action_receipts(event_id,subject_id,flow_epoch,occurred_at)
           SELECT gen_random_uuid(),subject_id,flow_epoch,now() FROM rec_subjects WHERE user_id={actor};
         INSERT INTO flow_book_daily(cohort_day,book_id,acquisition_hint,discovery_kind,cohorts,
             detail_opens,reader_opens,starts,completions,saves,follows,new_readers,d7_anchors,d7_returned,d7_unknown)
           VALUES((now() AT TIME ZONE 'UTC')::date-2,{book},1,3,2,2,2,2,2,0,0,0,2,0,2)"
    )).await
}
