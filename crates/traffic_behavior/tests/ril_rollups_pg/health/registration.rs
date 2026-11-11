use super::*;

async fn now_ms(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
        .fetch_one(&f.app).await.unwrap()
}
async fn epoch_count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ril_flow_health_epochs")
        .fetch_one(&f.app).await.unwrap()
}

#[tokio::test]
#[ignore = "requires fresh isolated PostgreSQL with keyed HTML health migration"]
async fn html_lost_registration_reply_returns_the_committed_epoch_and_persisted_baseline() {
    let f = Fixture::new().await;
    let health = PgHtmlHealth::new(f.app.clone());
    let key = Uuid::new_v4().simple().to_string();
    let from = now_ms(&f).await - 5_000;
    let original = health.register_epoch_after_gap(&key, None, from, (2, 3), (0, 0))
        .await.unwrap();
    // Treat the first committed response as lost. Counters grew while the
    // caller did not know the epoch and kept its producer disabled.
    let resumed = health.register_epoch_after_gap(&key, None, from - 100, (7, 9), (0, 0))
        .await.unwrap();
    assert_eq!(resumed, original);
    assert_eq!((resumed.dropped, resumed.failed), (2, 3),
        "new losses must not be silently baselined by a retry");
    assert_eq!(epoch_count(&f).await, 2, "one startup gap and one live epoch");
    health.note(resumed.epoch, 7, 9).await.unwrap();
    let stored: (i64, i64) = sqlx::query_as(
        "SELECT dropped,failed FROM ril_flow_health_epochs WHERE epoch=$1"
    ).bind(original.epoch).fetch_one(&f.app).await.unwrap();
    assert_eq!(stored, (7, 9));
    let flags: (bool, bool) = sqlx::query_as(
        "SELECT bool_or(had_loss),bool_or(had_error) FROM ril_flow_health_hourly WHERE epoch=$1"
    ).bind(original.epoch).fetch_one(&f.app).await.unwrap();
    assert_eq!(flags, (true, true));
    let key_again = health.register_epoch_after_gap(&key, None, from, (7, 9), (2, 3))
        .await.unwrap();
    assert_eq!(key_again.epoch, original.epoch);
    assert_eq!((key_again.dropped, key_again.failed), (7, 9));
    assert_eq!(epoch_count(&f).await, 2);
    assert_eq!(health.register_epoch_after_gap("not-a-key", None, from, (0, 0), (0, 0))
        .await, Err(HtmlHealthError::Invalid));
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires fresh isolated PostgreSQL with keyed HTML health migration"]
async fn html_concurrent_registration_and_timed_out_key_lock_never_create_an_orphan() {
    let f = Fixture::new().await;
    let health = PgHtmlHealth::new(f.app.clone());
    let key = Uuid::new_v4().simple().to_string();
    let from = now_ms(&f).await - 1_000;
    let (a, b) = tokio::join!(
        health.register_epoch_after_gap(&key, None, from, (0, 0), (0, 0)),
        health.register_epoch_after_gap(&key, None, from, (0, 0), (0, 0)),
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(epoch_count(&f).await, 2);
    let pending = Uuid::new_v4().simple().to_string();
    let mut lock = f.app.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,685))")
        .bind(&pending).execute(&mut *lock).await.unwrap();
    let started = Instant::now();
    assert_eq!(
        health.register_epoch_after_gap(&pending, None, from, (1, 1), (0, 0)).await,
        Err(HtmlHealthError::Unavailable),
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(epoch_count(&f).await, 2, "bounded failure registered nothing");
    lock.rollback().await.unwrap();
    let retried = health.register_epoch_after_gap(&pending, None, from, (1, 1), (0, 0))
        .await.unwrap();
    let same = health.register_epoch_after_gap(&pending, None, from, (1, 1), (0, 0))
        .await.unwrap();
    assert_eq!(retried, same);
    assert_eq!(epoch_count(&f).await, 4);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires fresh isolated PostgreSQL with keyed HTML health migration"]
async fn html_retry_of_committed_replacement_keeps_one_successor_and_the_original_unknown_gap() {
    let f = Fixture::new().await;
    let health = PgHtmlHealth::new(f.app.clone());
    let initial_key = Uuid::new_v4().simple().to_string();
    let from = now_ms(&f).await - 2_000;
    let initial = health.register_epoch_after_gap(&initial_key, None, from, (0, 0), (0, 0))
        .await.unwrap();
    sqlx::query("UPDATE ril_flow_health_epochs SET lease_until_ms=confirmed_ms WHERE epoch=$1")
        .bind(initial.epoch).execute(&f.app).await.unwrap();
    let next_key = Uuid::new_v4().simple().to_string();
    let next = health.register_epoch_after_gap(&next_key, Some(initial.epoch), from, (1, 1), (0, 0))
        .await.unwrap();
    let count = epoch_count(&f).await;
    assert_eq!(count, 4);
    let retry = health.register_epoch_after_gap(&next_key, Some(initial.epoch), from - 100, (2, 2), (0, 0))
        .await.unwrap();
    assert_eq!(retry, next);
    assert_eq!(epoch_count(&f).await, count);
    let old: (bool, i64) = sqlx::query_as(
        "SELECT retired,lease_until_ms FROM ril_flow_health_epochs WHERE epoch=$1"
    ).bind(initial.epoch).fetch_one(&f.app).await.unwrap();
    assert_eq!(old, (true, next.started_ms));
    let live: Vec<i64> = sqlx::query_scalar("SELECT epoch FROM ril_flow_health_epochs WHERE NOT retired")
        .fetch_all(&f.app).await.unwrap();
    assert_eq!(live, vec![next.epoch]);
    let gap: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ril_flow_health_epochs WHERE retired AND confirmed_ms=started_ms AND lease_until_ms=$1 AND started_ms<$1)"
    ).bind(next.started_ms).fetch_one(&f.app).await.unwrap();
    assert!(gap, "idempotent retry does not fill the earlier unknown interval");
    // The same attempt resolves an expired or later-retired epoch too. The
    // service learns its identity first, then chooses a fresh replacement key.
    health.retire(next.epoch, 1, 1, &HtmlIngressStopped::after_http_stopped_and_drained())
        .await.unwrap();
    let retired = health.register_epoch_after_gap(&next_key, Some(initial.epoch), from, (1, 1), (0, 0))
        .await.unwrap();
    assert_eq!(retired.epoch, next.epoch);
    assert_eq!(epoch_count(&f).await, count);
    f.finish().await;
}
