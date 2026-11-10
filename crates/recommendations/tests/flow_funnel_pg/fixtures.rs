use super::*;

#[derive(Clone, Copy)]
pub struct Book {
    pub id: i64,
    pub chapter: i64,
    pub version: i64,
}

pub struct Fixture {
    pub app: PgPool,
    owner: PgPool,
    pub visits: PgFlowVisits,
    pub funnel: PgFlowFunnel,
    pub rec: PgRecommendations,
    pub actor: i64,
    pub scope: FlowScope,
    schema: String,
}

impl Fixture {
    pub async fn new(name: &str) -> Self {
        let app_url = std::env::var("TEST_DATABASE_URL").expect("disposable app URL");
        let owner_url = std::env::var("MIGRATOR_URL").expect("disposable migrator URL");
        assert!(app_url.ends_with("/tor_ci") && app_url.contains("127.0.0.1:5432"));
        assert!(owner_url.ends_with("/tor_ci") && owner_url.contains("127.0.0.1:5432"));
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let schema = format!("flow_funnel_{name}");
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        for table in [
            "flow_visits",
            "flow_visit_receipts",
            "flow_visit_account_days",
            "flow_visit_daily",
            "flow_visit_continuations",
            "flow_book_cohorts",
            "flow_book_return_anchors",
            "flow_book_action_receipts",
            "flow_book_daily",
            "flow_book_worker_health",
            "event_deliveries",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        // LIKE copies checks/indexes, not foreign keys. Preserve the actual
        // anchor lifecycle while keeping the F read model isolated per test.
        sqlx::query(&format!("ALTER TABLE {schema}.flow_book_return_anchors
            ADD FOREIGN KEY(cohort_id,subject_id,flow_epoch,book_id)
            REFERENCES {schema}.flow_book_cohorts(id,subject_id,flow_epoch,book_id) ON DELETE CASCADE"))
            .execute(&owner).await.unwrap();
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&owner)
        .await
        .unwrap();
        let selected = schema.clone();
        let app = PgPoolOptions::new()
            .max_connections(6)
            .after_connect(move |connection, _| {
                let statement = format!("SET search_path={selected},public");
                Box::pin(async move {
                    sqlx::query(&statement).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        let actor = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('flow-f-'||gen_random_uuid()||'@example.invalid','FLOW F test','$argon2id$test') RETURNING id",
        ).fetch_one(&app).await.unwrap();
        let rec = PgRecommendations::new(app.clone());
        let enabled = rec
            .set_consent(actor, true, 0, &new_ticket().unwrap())
            .await
            .unwrap();
        rec.set_flow_consent(actor, true, enabled.revision, &new_ticket().unwrap())
            .await
            .unwrap();
        let scope = rec.flow_scope(actor).await.unwrap().unwrap();
        // An opted-in disposable actor permits historical timing fixtures.
        sqlx::query(
            "UPDATE rec_subjects SET flow_enabled_at=now()-interval '400 days' WHERE user_id=$1",
        )
        .bind(actor)
        .execute(&app)
        .await
        .unwrap();
        let visits = PgFlowVisits::new(app.clone());
        let funnel = PgFlowFunnel::new(app.clone());
        funnel.initialize(true).await.unwrap();
        Self {
            app,
            owner,
            visits,
            funnel,
            rec,
            actor,
            scope,
            schema,
        }
    }

    pub async fn now(&self) -> i64 {
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&self.app)
            .await
            .unwrap()
    }

    pub async fn book(&self) -> Book {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:flow-f:'||gen_random_uuid(),'ru','FLOW F book','public','basic') RETURNING id",
        ).fetch_one(&self.app).await.unwrap();
        self.chapter(id).await
    }

    pub async fn chapter(&self, book: i64) -> Book {
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
             SELECT $1,COALESCE(max(chapter_no),0)+1,'Chapter','public','free'
             FROM chapters WHERE book_id=$1 RETURNING id",
        )
        .bind(book)
        .fetch_one(&self.app)
        .await
        .unwrap();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,'abcdefghij') RETURNING id",
        )
        .bind(chapter)
        .fetch_one(&self.app)
        .await
        .unwrap();
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter)
            .bind(version)
            .execute(&self.app)
            .await
            .unwrap();
        Book {
            id: book,
            chapter,
            version,
        }
    }

    pub fn observation(&self, book: Book, kind: i16, at: i64, source: i16) -> VisitObservation {
        let mut observation =
            VisitObservation::new(self.scope.clone(), at, kind + 2, source).unwrap();
        observation.book = Some(FlowBookNavigation {
            book_id: book.id,
            kind,
            rec_assignment: None,
            organic_touch: None,
        });
        observation
    }

    pub async fn enter(&self, book: Book, kind: i16, seconds_ago: i64, source: i16) -> String {
        let event = self.observation(book, kind, self.now().await - seconds_ago * 1000, source);
        assert!(self.visits.observe(&event).await.unwrap());
        event.event_id
    }

    pub async fn canonical(
        &self,
        book: Book,
        kind: i16,
        seconds_ago: i64,
        enabled: bool,
    ) -> String {
        let id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
            .fetch_one(&self.app)
            .await
            .unwrap();
        let mut tx = self.app.begin().await.unwrap();
        sqlx::query("SELECT set_config('tor.flow_read_capture_enabled',$1,true)")
            .bind(if enabled { "1" } else { "0" })
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO outbox_events(id,event_type,schema_version,aggregate_kind,aggregate_id,
               dedupe_key,payload,required_consumers)
             VALUES($1::uuid,$6,1,'chapter',$2,$1,
               jsonb_build_object('user_id',$3::bigint,'book_id',$4::bigint,
                 'chapter_id',$2::bigint,'version_id',$5::bigint,'authority',$7::text),
               ARRAY['recommendation-feedback-v1'])",
        )
        .bind(&id)
        .bind(book.chapter)
        .bind(self.actor)
        .bind(book.id)
        .bind(book.version)
        .bind(if kind == 0 {
            "ChapterStartedV1"
        } else {
            "ChapterCompletedV1"
        })
        .bind(if kind == 0 {
            "C-verified-free-start"
        } else {
            "C-verified-free-position"
        })
        .execute(&mut *tx)
        .await
        .unwrap();
        let (table, time) = if kind == 0 {
            ("reader_chapter_starts", "started_at")
        } else {
            ("reader_chapter_completions", "completed_at")
        };
        sqlx::query(&format!(
            "INSERT INTO {table}(user_id,book_id,chapter_id,first_version_id,source_event_id,{time})
             VALUES($1,$2,$3,$4,$5::uuid,clock_timestamp()-$6::bigint*interval '1 second')",
        ))
        .bind(self.actor)
        .bind(book.id)
        .bind(book.chapter)
        .bind(book.version)
        .bind(&id)
        .bind(seconds_ago)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let proven: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM reader_flow_verified_read_events WHERE event_id=$1::uuid",
        )
        .bind(&id)
        .fetch_one(&self.app)
        .await
        .unwrap();
        assert_eq!(proven, 1);
        id
    }

    /// Move already committed synthetic evidence together; never forge a
    /// positive cohort or change a proof UUID/payload to make attribution pass.
    pub async fn age(&self, seconds: i64) {
        let mut tx = self.app.begin().await.unwrap();
        for sql in [
            "UPDATE flow_visits SET started_at=started_at-$2::bigint*interval '1 second',last_at=last_at-$2::bigint*interval '1 second',finalized_at=finalized_at-$2::bigint*interval '1 second' WHERE subject_id=$1::uuid",
            "UPDATE flow_visit_receipts SET observed_at=observed_at-$2::bigint*interval '1 second' WHERE subject_id=$1::uuid",
            "UPDATE flow_book_cohorts SET visit_started_at=visit_started_at-$2::bigint*interval '1 second',first_entry_at=first_entry_at-$2::bigint*interval '1 second',expires_at=expires_at-$2::bigint*interval '1 second',first_start_at=first_start_at-$2::bigint*interval '1 second',finalized_at=finalized_at-$2::bigint*interval '1 second' WHERE subject_id=$1::uuid",
            "UPDATE flow_book_return_anchors SET started_at=started_at-$2::bigint*interval '1 second',expires_at=expires_at-$2::bigint*interval '1 second' WHERE subject_id=$1::uuid",
            "UPDATE reader_flow_read_receipts SET occurred_at=occurred_at-$2::bigint*interval '1 second' WHERE subject_id=$1::uuid",
            "UPDATE reader_flow_book_actions SET occurred_at=occurred_at-$2::bigint*interval '1 second' WHERE subject_id=$1::uuid",
        ] {
            sqlx::query(sql).bind(&self.scope.subject_id).bind(seconds).execute(&mut *tx).await.unwrap();
        }
        for sql in [
            "UPDATE reader_chapter_starts SET started_at=started_at-$2::bigint*interval '1 second' WHERE user_id=$1",
            "UPDATE reader_chapter_completions SET completed_at=completed_at-$2::bigint*interval '1 second' WHERE user_id=$1",
        ] {
            sqlx::query(sql).bind(self.actor).bind(seconds).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
    }

    pub async fn counts(&self) -> FlowFunnelCounts {
        self.funnel
            .report(90)
            .await
            .unwrap()
            .current
            .counts
            .unwrap_or_default()
    }

    pub async fn scalar(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.app).await.unwrap()
    }

    pub async fn deliveries(&self, consumer: Option<&str>) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
             WHERE e.payload->>'user_id'=$1 AND ($2::text IS NULL OR d.consumer=$2)",
        )
        .bind(self.actor.to_string())
        .bind(consumer)
        .fetch_one(&self.app)
        .await
        .unwrap()
    }

    pub async fn disable(&self) {
        let consent = self.rec.flow_consent(self.actor).await.unwrap();
        self.rec
            .set_flow_consent(self.actor, false, consent.revision, &new_ticket().unwrap())
            .await
            .unwrap();
    }

    pub async fn cleanup(self) {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.actor)
            .execute(&self.app)
            .await
            .unwrap();
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

pub fn feed(book: i64) -> Feed {
    Feed {
        items: vec![FeedItem {
            book_id: book.to_string(),
            title: "FLOW F book".into(),
            reason: "similar_by_content".into(),
            source: "content".into(),
            algorithm_version: ALGORITHM,
            ticket: None,
            feedback_revision: Some(0),
        }],
        generation_id: None,
        next_cursor: None,
        personalized: true,
    }
}
