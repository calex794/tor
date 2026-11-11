//! Current installed schema and real anonymous proof/aggregate transactions.
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_contracts::exposure::{
    CardObservation, EditorialPlacement, ExposureSurface, PromotionSlot,
};
use tor_recommendations::{
    domain::exposure::*,
    repositories::{exposure::PgExposure, PgRecommendations},
    services::exposure::{digest, issue},
};
#[path = "support/current_analytics.rs"]
mod current_analytics;
#[path = "exposure_pg/fixtures.rs"]
mod fixtures;
#[path = "exposure_pg/report.rs"]
mod report;
use fixtures::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn events_are_atomic_restart_safe_and_never_create_reader_identity() {
    let f = Fixture::new(2).await;
    let (proof, issued) = issue(f.card(0)).unwrap();
    assert_eq!(
        f.store
            .register(std::slice::from_ref(&issued))
            .await
            .unwrap(),
        1
    );
    assert_eq!(f.store.register(&[issued]).await.unwrap(), 0);
    let exact_clock:bool=sqlx::query_scalar("SELECT expires_at=issued_at+interval '15 minutes' AND bucket=date_trunc('hour',issued_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' FROM flow_observation_proofs WHERE digest=$1")
        .bind(digest(&proof).unwrap().as_slice()).fetch_one(&f.pool).await.unwrap();
    assert!(
        exact_clock,
        "issue, expiry and UTC cohort must share one database instant"
    );
    let context = f.store.describe(&proof).await.unwrap();
    let click = event(&proof, EventKind::Click);
    assert_eq!(
        f.store.record(&click, &context, None).await.unwrap(),
        ObservationReceipt::Recorded
    );
    assert_eq!(f.counts(0).await, (1, 1, 0, 1, 0));
    // A click never manufactures a view, and later DOM/view callbacks may reorder.
    assert_eq!(
        f.store
            .record(&event(&proof, EventKind::Rendered), &context, None)
            .await
            .unwrap(),
        ObservationReceipt::Replay
    );
    let mut invalid = event(&proof, EventKind::View);
    invalid.visible_percent = 59;
    assert_eq!(
        f.store.record(&invalid, &context, None).await,
        Err(ExposureError::Invalid)
    );
    invalid.visible_percent = 60;
    invalid.foreground = false;
    assert_eq!(
        f.store.record(&invalid, &context, None).await,
        Err(ExposureError::Invalid)
    );
    let forged = ProofContext { card: f.card(1) };
    assert_eq!(
        f.store.record(&click, &forged, None).await,
        Err(ExposureError::Forbidden)
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let view = event(&proof, EventKind::View);
    let (a, b) = tokio::join!(
        f.store.record(&view, &context, None),
        f.store.record(&view, &context, None)
    );
    assert!([a.unwrap(), b.unwrap()].contains(&ObservationReceipt::Recorded));
    assert_eq!(f.counts(0).await, (1, 1, 1, 1, 2));
    let restarted = PgExposure::new(f.pool.clone());
    assert_eq!(
        restarted
            .record(&view, &restarted.describe(&proof).await.unwrap(), None)
            .await
            .unwrap(),
        ObservationReceipt::Replay
    );
    assert_eq!(f.counts(0).await, (1, 1, 1, 1, 3));
    let bad = sqlx::query("UPDATE flow_observation_proofs SET book_id=$2 WHERE digest=$1")
        .bind(digest(&proof).unwrap().as_slice())
        .bind(f.books[1])
        .execute(&f.pool)
        .await
        .unwrap_err();
    assert_eq!(
        bad.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    let subject: i64 = sqlx::query_scalar("SELECT count(*) FROM rec_subjects WHERE user_id=$1")
        .bind(f.actor)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(subject, 0);
    let forbidden:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_name IN ('flow_observation_proofs','flow_observation_hourly') AND column_name ~ '(user|actor|subject|session|cookie|ip_address|referrer|url)'").fetch_one(&f.pool).await.unwrap();
    assert_eq!(forbidden, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn issuer_readiness_requires_its_schema_and_does_not_wait_for_locked_database() {
    let f = Fixture::new(1).await;
    assert!(f.store.available().await);
    let untouched = current_analytics::url("TEST_DATABASE_URL", "tor_app")
        .strip_suffix("/tor_ci")
        .unwrap()
        .to_owned()
        + "/postgres";
    let absent = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(&untouched)
        .unwrap();
    assert!(
        !PgExposure::new(absent.clone()).available().await,
        "an unmigrated database must expose no issuer"
    );
    absent.close().await;
    let mut held = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE flow_observation_state IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    assert!(!f.store.available().await);
    assert!(started.elapsed() < Duration::from_millis(500));
    held.rollback().await.unwrap();
    assert!(f.store.available().await);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn live_book_age_explicit_choices_and_database_contention_fail_closed() {
    let f = Fixture::new(1).await;
    let (proof, issued) = issue(f.card(0)).unwrap();
    f.store.register(&[issued]).await.unwrap();
    let context = f.store.describe(&proof).await.unwrap();
    let click = event(&proof, EventKind::Click);
    f.book_change("admin_hold=true").await;
    assert_eq!(
        f.store.record(&click, &context, None).await,
        Err(ExposureError::Forbidden)
    );
    f.book_change("admin_hold=false,access_level='adult'").await;
    assert_eq!(
        f.store.record(&click, &context, None).await,
        Err(ExposureError::Forbidden)
    );
    assert_eq!(
        f.store.record(&click, &context, Some(f.actor)).await,
        Err(ExposureError::Forbidden)
    );
    sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
        .bind(f.actor)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .record(&click, &context, Some(f.actor))
            .await
            .unwrap(),
        ObservationReceipt::Recorded
    );
    PgRecommendations::new(f.pool.clone())
        .set_feedback(
            f.actor,
            f.books[0],
            tor_recommendations::domain::feedback::Choice::NotInterested,
            0,
            &issue(f.card(0)).unwrap().0,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.record(&click, &context, Some(f.actor)).await,
        Err(ExposureError::Forbidden)
    );
    f.book_change("access_level='basic'").await;
    let (second, item) = issue(f.card(0)).unwrap();
    f.store.register(&[item]).await.unwrap();
    let ctx = f.store.describe(&second).await.unwrap();
    let mut lock = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE flow_observation_hourly IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let before = std::time::Instant::now();
    let result = f
        .store
        .record(&event(&second, EventKind::Click), &ctx, None)
        .await;
    assert_eq!(result, Err(ExposureError::Unavailable));
    assert!(before.elapsed() < Duration::from_secs(1));
    lock.rollback().await.unwrap();
    assert_eq!(f.counts(0).await, (2, 1, 0, 1, 0));
    let accepted: bool =
        sqlx::query_scalar("SELECT clicked FROM flow_observation_proofs WHERE digest=$1")
            .bind(digest(&second).unwrap().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(!accepted);
    assert_eq!(
        f.store
            .record(&event(&second, EventKind::Click), &ctx, None)
            .await
            .unwrap(),
        ObservationReceipt::Recorded
    );
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(f.actor)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store.record(&click, &context, Some(f.actor)).await,
        Err(ExposureError::Forbidden)
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn short_lived_state_has_hard_capacity_and_cleanup_is_bounded() {
    let f = Fixture::new(1).await;
    let mut tx = f.pool.begin().await.unwrap();
    let original: i64 = sqlx::query_scalar(
        "SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let fill = 100_000 - original;
    assert!(fill > 1000);
    sqlx::query("INSERT INTO flow_observation_proofs(digest,book_id,surface,issued_at,expires_at) SELECT decode(md5($1::text||':'||v::text)||md5(v::text||':'||$1::text),'hex'),$1,'home',now()-interval '16 minutes',now()-interval '1 minute' FROM generate_series(1,$2::bigint) v").bind(f.books[0]).bind(fill).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities) VALUES(date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC',$1,'home',$2)").bind(f.books[0]).bind(fill).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE flow_observation_state SET live_proofs=100000 WHERE singleton")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (_, new) = issue(f.card(0)).unwrap();
    assert_eq!(f.store.register(&[new]).await, Err(ExposureError::Capacity));
    assert_eq!(f.store.prune().await.unwrap(), 1000);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT live_proofs FROM flow_observation_state WHERE singleton"
        )
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        99_000
    );
    let expired: i64 = sqlx::query_scalar(
        "SELECT sum(expired_unobserved)::bigint FROM flow_observation_hourly WHERE book_id=$1",
    )
    .bind(f.books[0])
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(expired, 1000);
    let counts = f.store.report(&f.filter()).await.unwrap();
    assert_eq!(counts.current.known_views(), None);
    assert_eq!(counts.low_observed_books, 0);
    f.cleanup().await;
}
