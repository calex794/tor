//! Real disposable PostgreSQL tests for optional visit scope, receipts and reports.
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use tor_recommendations::{
    repositories::{visits::PgFlowVisits, PgRecommendations},
    services::{
        feedback::new_ticket,
        visits::{FlowScope, FlowVisitError, VisitObservation},
    },
};

#[path = "support/legacy_analytics.rs"]
mod legacy_analytics;

struct Fixture {
    app: PgPool,
    owner: PgPool,
    store: PgFlowVisits,
    rec: PgRecommendations,
    actor: i64,
    scope: FlowScope,
    schema: String,
}

impl Fixture {
    async fn new(name: &str) -> Self {
        let app_url = legacy_analytics::url("LEGACY_ANALYTICS_DATABASE_URL", "tor_app");
        let owner_url =
            legacy_analytics::url("LEGACY_ANALYTICS_MIGRATOR_URL", "tor_migrator");
        let owner = PgPool::connect(&owner_url).await.unwrap();
        legacy_analytics::assert_schema(&owner).await;
        let schema = format!("flow_visits_{name}");
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
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
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
            .max_connections(5)
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
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'FLOW visit test','$argon2id$test') RETURNING id",
        )
        .bind(format!("flow-{}@example.invalid", new_ticket().unwrap()).to_ascii_lowercase())
        .fetch_one(&app)
        .await
        .unwrap();
        let rec = PgRecommendations::new(app.clone());
        let enabled = rec
            .set_consent(actor, true, 0, &new_ticket().unwrap())
            .await
            .unwrap();
        assert!(rec.flow_scope(actor).await.unwrap().is_none());
        rec.set_flow_consent(actor, true, enabled.revision, &new_ticket().unwrap())
            .await
            .unwrap();
        let scope = rec.flow_scope(actor).await.unwrap().unwrap();
        // Only a synthetic opted-in fixture is backdated for event-time cases.
        sqlx::query(
            "UPDATE rec_subjects SET flow_enabled_at=now()-interval '4 hours' WHERE user_id=$1",
        )
        .bind(actor)
        .execute(&app)
        .await
        .unwrap();
        let store = PgFlowVisits::new(app.clone());
        Self {
            app,
            owner,
            store,
            rec,
            actor,
            scope,
            schema,
        }
    }

    async fn now(&self) -> i64 {
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&self.app)
            .await
            .unwrap()
    }

    fn event(&self, at: i64, surface: i16, source: i16) -> VisitObservation {
        VisitObservation::new(self.scope.clone(), at, surface, source).unwrap()
    }

    async fn raw_counts(&self) -> (i64, i64, i64) {
        sqlx::query_as(
            "SELECT count(*),COALESCE(sum(pageviews),0)::bigint,
               (SELECT count(*) FROM flow_visit_receipts) FROM flow_visits",
        )
        .fetch_one(&self.app)
        .await
        .unwrap()
    }

    async fn historical(&self, at: i64, surface: i16, source: i16) {
        let event = self.event(at, surface, source);
        sqlx::query(
            "INSERT INTO flow_visits(id,subject_id,flow_epoch,started_at,last_at,
                 first_event_id,last_event_id,landing_surface,last_surface,acquisition_hint,pageviews)
             VALUES($1::uuid,$2::uuid,$3::uuid,to_timestamp($4::double precision/1000),
                 to_timestamp($4::double precision/1000),$1::uuid,$1::uuid,$5,$5,$6,1)",
        )
        .bind(&event.event_id)
        .bind(&self.scope.subject_id)
        .bind(&self.scope.epoch)
        .bind(at)
        .bind(surface)
        .bind(source)
        .execute(&self.app)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
             VALUES($1::uuid,$2::uuid,(to_timestamp($3::double precision/1000) AT TIME ZONE 'UTC')::date)
             ON CONFLICT DO NOTHING",
        )
        .bind(&self.scope.subject_id)
        .bind(&self.scope.epoch)
        .bind(at)
        .execute(&self.app)
        .await
        .unwrap();
    }

    async fn cleanup(self) {
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn flow_visits_replay_reorder_concurrency_restart_and_invalid_observations() {
    let f = Fixture::new("capture").await;
    let now = f.now().await;
    let first = f.event(now - 180_000, 0, 0);
    let second = f.event(now - 100_000, 2, 3);
    assert!(f.store.observe(&first).await.unwrap());
    assert!(f.store.observe(&second).await.unwrap());
    let before = f.event(now - 240_000, 1, 1);
    assert!(f.store.observe(&before).await.unwrap());
    let last = f.event(now, 3, 3);
    let restarted = PgFlowVisits::new(f.app.clone());
    let (a, b) = tokio::join!(f.store.observe(&last), restarted.observe(&last));
    assert_eq!(i32::from(a.unwrap()) + i32::from(b.unwrap()), 1);
    assert!(!restarted.observe(&first).await.unwrap());
    assert_eq!(f.raw_counts().await, (1, 4, 4));
    let row = sqlx::query("SELECT landing_surface,last_surface,acquisition_hint FROM flow_visits")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(row.get::<i16, _>("landing_surface"), 1);
    assert_eq!(row.get::<i16, _>("last_surface"), 3);
    assert_eq!(row.get::<i16, _>("acquisition_hint"), 1);
    let mut conflicting = first.clone();
    conflicting.surface = 4;
    assert_eq!(
        f.store.observe(&conflicting).await,
        Err(FlowVisitError::Conflict)
    );
    assert!(!f
        .store
        .observe(&f.event(now - 301_000, 0, 0))
        .await
        .unwrap());
    assert!(!f.store.observe(&f.event(now + 60_000, 0, 0)).await.unwrap());
    let mut forged = first.clone();
    forged.event_id = "not-a-uuid".into();
    assert_eq!(f.store.observe(&forged).await, Err(FlowVisitError::Invalid));
    let report = f.store.report(7).await.unwrap();
    assert_eq!(report.current.visits, Some(1));
    assert_eq!(report.current.pageviews, Some(4));
    assert_eq!(report.current.unique_subjects, Some(1));
    assert_eq!(report.current.observed_seconds, Some(240));
    assert_eq!(report.previous.visits, None);
    assert_eq!(report.previous.unique_subjects, None);
    assert_eq!(f.store.report(2).await, Err(FlowVisitError::Invalid));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn flow_visits_exact_idle_boundary_and_late_bridge_merge_existing_windows() {
    let f = Fixture::new("bridge").await;
    let now = f.now().await;
    f.historical(now - 32 * 60_000, 0, 0).await;
    let next = f.event(now - 2 * 60_000, 3, 3);
    assert!(f.store.observe(&next).await.unwrap());
    assert_eq!(
        f.raw_counts().await.0,
        2,
        "exactly 30 minutes starts a new window"
    );
    let bridge = f.event(now - 3 * 60_000, 2, 1);
    assert!(f.store.observe(&bridge).await.unwrap());
    assert_eq!(f.raw_counts().await, (1, 3, 2));
    let report = f.store.report(7).await.unwrap();
    assert_eq!(report.current.rows.len(), 1);
    let row = &report.current.rows[0];
    assert_eq!(
        (row.acquisition_hint, row.landing_surface, row.last_surface),
        (0, 0, 3)
    );
    assert_eq!(row.observed_seconds, 30 * 60);
    // Repeating maintenance while still open cannot anonymize or duplicate it.
    f.store.maintain().await.unwrap();
    assert_eq!(f.store.report(7).await.unwrap().current.visits, Some(1));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn flow_visits_finalize_replay_revoke_and_expiry_keep_only_anonymous_history() {
    let f = Fixture::new("retention").await;
    let now = f.now().await;
    f.historical(now - 40 * 60_000, 0, 0).await;
    f.historical(now - 31 * 86_400_000, 1, 1).await;
    f.store.maintain().await.unwrap();
    let before = f.store.report(90).await.unwrap();
    assert_eq!(before.current.visits, Some(2));
    assert_eq!(before.current.unique_subjects, None);
    assert_eq!(before.previous.unique_subjects, None);
    assert_eq!(f.raw_counts().await.0, 1, "old personal visit is erased");
    f.store.maintain().await.unwrap();
    assert_eq!(f.store.report(90).await.unwrap().current, before.current);
    let current = f.rec.flow_consent(f.actor).await.unwrap();
    f.rec
        .set_flow_consent(f.actor, false, current.revision, &new_ticket().unwrap())
        .await
        .unwrap();
    assert_eq!(f.raw_counts().await, (0, 0, 0));
    let after = f.store.report(90).await.unwrap();
    assert_eq!(
        after.current.visits,
        Some(1),
        "only already anonymous old total remains"
    );
    let recent = f.store.report(7).await.unwrap();
    assert_eq!(recent.current.visits, Some(0));
    assert_eq!(recent.current.unique_subjects, None);
    assert_eq!(recent.current.unattributed_pageviews, None);
    assert!(!f
        .store
        .observe(&f.event(f.now().await, 0, 0))
        .await
        .unwrap());
    // Anonymous daily data has its own hard 180-calendar-day retention.
    sqlx::query("UPDATE flow_visit_daily SET cohort_day=(now() AT TIME ZONE 'UTC')::date-181")
        .execute(&f.app)
        .await
        .unwrap();
    f.store.maintain().await.unwrap();
    assert_eq!(f.store.report(90).await.unwrap().current.visits, None);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn flow_visits_distinct_accounts_use_observed_days_not_visit_start_or_daily_sums() {
    let f = Fixture::new("distinct").await;
    let now = f.now().await;
    f.historical(now - 2 * 86_400_000, 1, 3).await;
    sqlx::query(
        "INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
         SELECT $1::uuid,$2::uuid,(now() AT TIME ZONE 'UTC')::date-day_offset
         FROM generate_series(0,2) AS day_offset ON CONFLICT DO NOTHING",
    )
    .bind(&f.scope.subject_id)
    .bind(&f.scope.epoch)
    .execute(&f.app)
    .await
    .unwrap();
    let today = f.store.report(1).await.unwrap();
    assert_eq!(today.current.visits, None);
    assert_eq!(today.current.unique_subjects, Some(1));
    assert_eq!(today.previous.unique_subjects, Some(1));
    let week = f.store.report(7).await.unwrap();
    assert_eq!(
        week.current.unique_subjects,
        Some(1),
        "one subject across all days"
    );
    assert_eq!(week.current.visits, Some(1));
    let month = f.store.report(30).await.unwrap();
    assert_eq!(month.current.unique_subjects, Some(1));
    assert_eq!(month.previous.unique_subjects, None);
    let quarter = f.store.report(90).await.unwrap();
    assert_eq!(quarter.current.unique_subjects, None);
    // UTC boundaries do not inherit a PostgreSQL connection's local timezone.
    sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
        .execute(&f.app)
        .await
        .unwrap();
    assert_eq!(f.store.report(7).await.unwrap().current, week.current);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn flow_visits_concurrent_opt_out_erases_late_capture_and_rotated_scope() {
    let f = Fixture::new("revoke").await;
    f.historical(f.now().await - 40 * 60_000, 0, 0).await;
    let event = f.event(f.now().await, 2, 1);
    let consent = f.rec.flow_consent(f.actor).await.unwrap();
    let command = new_ticket().unwrap();
    let (capture, revoke, maintenance) = tokio::join!(
        f.store.observe(&event),
        f.rec
            .set_flow_consent(f.actor, false, consent.revision, &command),
        f.store.maintain(),
    );
    capture.unwrap();
    maintenance.unwrap();
    let revoked = revoke.unwrap();
    assert_eq!(f.raw_counts().await, (0, 0, 0));
    assert_eq!(
        f.store.report(7).await.unwrap().current.visits.unwrap_or(0),
        0
    );
    assert!(!f.store.observe(&event).await.unwrap());
    f.rec
        .set_flow_consent(f.actor, true, revoked.revision, &new_ticket().unwrap())
        .await
        .unwrap();
    let new_scope = f.rec.flow_scope(f.actor).await.unwrap().unwrap();
    assert_ne!(f.scope.epoch, new_scope.epoch);
    assert!(!f.store.observe(&event).await.unwrap());
    let fresh = VisitObservation::new(new_scope, f.now().await, 4, 3).unwrap();
    assert!(f.store.observe(&fresh).await.unwrap());
    assert_eq!(
        f.store.report(1).await.unwrap().current.unique_subjects,
        Some(1)
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn flow_visits_long_continuation_erases_origin_without_a_false_new_start() {
    let f = Fixture::new("continuation").await;
    let now = f.now().await;
    f.historical(now - 31 * 86_400_000, 0, 0).await;
    // A deliberately extreme fixture has no idle gap over its entire lifetime.
    sqlx::query(
        "UPDATE flow_visits SET last_at=to_timestamp($1::double precision/1000),pageviews=2",
    )
    .bind(now - 32 * 60_000)
    .execute(&f.app)
    .await
    .unwrap();
    f.store.maintain().await.unwrap();
    assert_eq!(f.raw_counts().await.0, 0);
    let marker: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_visit_continuations")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(marker, 1);
    let after_gap = f.event(now - 2 * 60_000, 2, 3);
    assert!(f.store.observe(&after_gap).await.unwrap());
    assert_eq!(
        f.raw_counts().await.0,
        1,
        "initially observed exact idle boundary"
    );
    // A delayed event bridges the anonymous old visit and the apparent new one.
    // The old source stays erased; the false start must be retracted.
    let late_bridge = f.event(now - 3 * 60_000, 1, 1);
    assert!(f.store.observe(&late_bridge).await.unwrap());
    assert_eq!(f.raw_counts().await, (0, 0, 2));
    let report = f.store.report(90).await.unwrap();
    assert_eq!(report.current.visits, Some(1));
    let recent = f.store.report(7).await.unwrap();
    assert_eq!(recent.current.visits, None);
    assert_eq!(recent.current.unique_subjects, Some(1));
    assert_eq!(recent.current.unattributed_pageviews, Some(2));
    assert!(!f.store.observe(&late_bridge).await.unwrap());
    assert_eq!(
        f.store
            .report(7)
            .await
            .unwrap()
            .current
            .unattributed_pageviews,
        Some(2)
    );
    // Once both idle and admissible delivery delay have passed, the minimal
    // timestamp can be erased. A later genuine new window is counted normally.
    sqlx::query("UPDATE flow_visit_continuations SET last_at=now()-interval '36 minutes'")
        .execute(&f.app)
        .await
        .unwrap();
    f.store.maintain().await.unwrap();
    assert!(f
        .store
        .observe(&f.event(f.now().await, 4, 3))
        .await
        .unwrap());
    assert_eq!(f.store.report(7).await.unwrap().current.visits, Some(1));
    assert_eq!(f.store.report(90).await.unwrap().current.visits, Some(2));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires explicit legacy analytics PostgreSQL baseline 191"]
async fn flow_visits_archive_totals_do_not_imply_remaining_identity_or_receipt_evidence() {
    let f = Fixture::new("archive_raw").await;
    sqlx::query(
        "INSERT INTO flow_visit_daily
         (cohort_day,acquisition_hint,landing_surface,last_surface,visits,pageviews,observed_seconds)
         VALUES((now() AT TIME ZONE 'UTC')::date,3,1,2,7,11,90),
               ((now() AT TIME ZONE 'UTC')::date-1,3,1,2,3,5,60)",
    )
    .execute(&f.app)
    .await
    .unwrap();
    let archive = f.store.report(1).await.unwrap();
    assert_eq!(archive.current.visits, Some(7));
    assert_eq!(archive.current.pageviews, Some(11));
    assert_eq!(archive.current.observed_seconds, Some(90));
    assert_eq!(archive.previous.visits, Some(3));
    assert_eq!(archive.previous.pageviews, Some(5));
    assert_eq!(archive.previous.observed_seconds, Some(60));
    for period in [&archive.current, &archive.previous] {
        assert_eq!(period.unique_subjects, None);
        assert_eq!(period.unattributed_pageviews, None);
    }

    // A genuine retained observation still proves one account and zero
    // continuation pageviews; the anonymous archive alone proves neither.
    assert!(f
        .store
        .observe(&f.event(f.now().await, 2, 1))
        .await
        .unwrap());
    let observed = f.store.report(1).await.unwrap();
    assert_eq!(observed.current.unique_subjects, Some(1));
    assert_eq!(observed.current.unattributed_pageviews, Some(0));
    assert_eq!(observed.current.visits, Some(8));
    assert_eq!(observed.current.pageviews, Some(12));

    // The retirement batch removes receipts before account days. A surviving
    // account day proves distinct accounts, never zero continuation pageviews.
    sqlx::query("DELETE FROM flow_visit_receipts")
        .execute(&f.app)
        .await
        .unwrap();
    let without_receipts = f.store.report(1).await.unwrap();
    assert_eq!(without_receipts.current.unique_subjects, Some(1));
    assert_eq!(without_receipts.current.unattributed_pageviews, None);

    // Another genuine observation restores receipts. Removing only the account
    // ledger must not hide its independently retained continuation measurement.
    assert!(f
        .store
        .observe(&f.event(f.now().await, 3, 1))
        .await
        .unwrap());
    sqlx::query("DELETE FROM flow_visit_account_days")
        .execute(&f.app)
        .await
        .unwrap();
    let without_accounts = f.store.report(1).await.unwrap();
    assert_eq!(without_accounts.current.unique_subjects, None);
    assert_eq!(without_accounts.current.unattributed_pageviews, Some(0));
    sqlx::query("DELETE FROM flow_visit_receipts")
        .execute(&f.app)
        .await
        .unwrap();
    let without_raw_proof = f.store.report(1).await.unwrap();
    assert_eq!(without_raw_proof.current.unique_subjects, None);
    assert_eq!(without_raw_proof.current.unattributed_pageviews, None);
    assert_eq!(without_raw_proof.current.visits, Some(8));
    assert_eq!(without_raw_proof.current.pageviews, Some(13));
    assert_eq!(without_raw_proof.previous, archive.previous);
    f.cleanup().await;
}
