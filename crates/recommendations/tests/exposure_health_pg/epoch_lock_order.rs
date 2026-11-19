//! A slow aggregate update must not pin the worker's epoch row.
//! This reproduces the checkpoint/callback lock convoy from FLOW #1134 with
//! real disposable PostgreSQL 18, never a mocked lease or a relaxed timeout.
use super::*;
use std::time::{Duration, Instant};

async fn slow_hourly_update(f: &Fixture) {
    sqlx::query(&format!(
        "CREATE FUNCTION {}.flow_test_pause_aggregate() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN PERFORM pg_sleep(0.30); RETURN NEW; END $$",
        f.schema
    ))
    .execute(&f.admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        "CREATE TRIGGER flow_test_pause_hourly BEFORE UPDATE
         ON {}.flow_observation_hourly FOR EACH ROW
         EXECUTE FUNCTION {}.flow_test_pause_aggregate()",
        f.schema, f.schema
    ))
    .execute(&f.admin)
    .await
    .unwrap();
}

// PostgreSQL exposes the currently executing SQL while the trigger is in
// pg_sleep. Observe it instead of guessing how long a CI runner takes to start.
async fn during_aggregate_update(f: &Fixture) {
    let started = Instant::now();
    loop {
        let sleeping: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database()
                  AND state='active' AND wait_event='PgSleep'
                  AND query LIKE 'UPDATE flow_observation_hourly h SET%'
            )",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        if sleeping {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the production callback never entered the controlled aggregate update"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn aggregate_wait_cannot_starve_checkpoint_or_commit_after_epoch_handoff() {
    let f = fixture().await;
    sqlx::query("INSERT INTO flow_observation_state(singleton) VALUES(true)")
        .execute(&f.pool)
        .await
        .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO public.books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','FLOW epoch lock order','public','basic') RETURNING id",
    )
    .bind(format!("ci:flow-epoch-lock:{}", f.schema))
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let health = PgExposureHealth::new(f.pool.clone());
    let store = PgExposure::new(f.pool.clone());
    let now: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let epoch = health
        .start_epoch(
            &worker_registration_key().unwrap(),
            None,
            now,
            HealthCounters::default(),
        )
        .await
        .unwrap()
        .epoch;
    let card = CardObservation {
        book_id: book,
        surface: ExposureSurface::Home,
        editorial: None,
    };
    let (proof, issued) = issue(card).unwrap();
    assert_eq!(store.register_in_epoch(&[issued], epoch).await.unwrap(), 1);
    let context = store.describe(&proof).await.unwrap();
    slow_hourly_update(&f).await;

    // The callback is already editing a real aggregate row for 300ms. The
    // checkpoint has only a 75ms PG lock budget and must not wait on that row.
    let first_store = PgExposure::new(f.pool.clone());
    let first_context = context.clone();
    let first_proof = proof.clone();
    let first = tokio::spawn(async move {
        first_store
            .record_in_epoch(
                &ObservationEvent {
                    proof: first_proof,
                    kind: EventKind::Rendered,
                    visible_percent: 0,
                    continuous_visible_ms: 0,
                    foreground: true,
                },
                &first_context,
                None,
                epoch,
            )
            .await
    });
    during_aggregate_update(&f).await;
    let cut = tokio::time::timeout(Duration::from_millis(150), health.begin_checkpoint(epoch))
        .await
        .expect("checkpoint blocked behind an unrelated hourly aggregate")
        .expect("a live checkpoint must keep its lease");
    assert_eq!(first.await.unwrap(), Ok(ObservationReceipt::Recorded));
    let (_, complete) = health
        .finish_checkpoint(cut, 1, 1, HealthCounters::default())
        .await
        .unwrap();
    assert!(complete);

    // A predecessor handoff while another callback is in the aggregate must
    // win the epoch lock. Its old callback then rolls back every partial write.
    let later_store = PgExposure::new(f.pool.clone());
    let later_context = context.clone();
    let second = tokio::spawn(async move {
        later_store
            .record_in_epoch(
                &ObservationEvent {
                    proof,
                    kind: EventKind::Click,
                    visible_percent: 0,
                    continuous_visible_ms: 0,
                    foreground: true,
                },
                &later_context,
                None,
                epoch,
            )
            .await
    });
    during_aggregate_update(&f).await;
    let successor = health
        .start_epoch(
            &worker_registration_key().unwrap(),
            Some(epoch),
            now,
            HealthCounters::default(),
        )
        .await
        .expect("epoch handoff blocked behind unrelated hourly aggregate");
    assert!(successor.epoch > epoch);
    assert_eq!(second.await.unwrap(), Err(ExposureError::Unavailable));
    let tallies: (i64, i64, i64) = sqlx::query_as(
        "SELECT opportunities,rendered,clicks FROM flow_observation_hourly WHERE book_id=$1",
    )
    .bind(book)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        tallies,
        (1, 1, 0),
        "retired callback must leave no partial click"
    );
    sqlx::query("DELETE FROM public.books WHERE id=$1")
        .bind(book)
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}
