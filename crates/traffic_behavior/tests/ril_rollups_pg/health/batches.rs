use super::*;

#[tokio::test]
#[ignore = "requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_ack_requires_the_materialized_batch_and_fences_replay_and_expired_leases() {
    let f = Fixture::new().await;
    let health = PgHtmlHealth::new(f.app.clone());
    let ril = PgRilRollups::new(f.app.clone());
    assert!(health.ready().await.unwrap());
    let (epoch, clock) = health.start_epoch().await.unwrap();
    sqlx::query("UPDATE ril_flow_health_epochs SET started_ms=$2-10000,confirmed_ms=$2-10000 WHERE epoch=$1").bind(epoch).bind(clock).execute(&f.app).await.unwrap();
    let point = health.begin(epoch).await.unwrap();
    let mut delta = RollupDelta::default();
    delta.record(event(), None);
    delta.record_observed_flow(page(), Some(f.hour().await));
    let id = Uuid::new_v4().to_string();
    ril.enqueue(&id, &delta).await.unwrap();
    assert!(!health.finish(point, Some(&id), 0, 0).await.unwrap());
    let confirmed: i64 =
        sqlx::query_scalar("SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1")
            .bind(epoch)
            .fetch_one(&f.app)
            .await
            .unwrap();
    assert_eq!(confirmed, clock - 10000, "staging is not materialization");
    health.renew(point).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1"
        )
        .bind(epoch)
        .fetch_one(&f.app)
        .await
        .unwrap(),
        confirmed
    );
    // Failure evidence survives a reader restart even before the batch settles.
    health.note(epoch, 2, 1).await.unwrap();
    let flags: (bool, bool) = sqlx::query_as(
        "SELECT bool_or(had_loss),bool_or(had_error) FROM ril_flow_health_hourly WHERE epoch=$1",
    )
    .bind(epoch)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(flags, (true, true));
    assert_eq!(
        health.note(epoch, 1, 1).await,
        Err(HtmlHealthError::Invalid)
    );
    let claim = ril.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id, id);
    assert!(ril.materialize(&claim).await.unwrap());
    let (a, b) = tokio::join!(
        health.finish(point, Some(&id), 2, 1),
        health.finish(point, Some(&id), 2, 1)
    );
    assert!(a.unwrap() && b.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1"
        )
        .bind(epoch)
        .fetch_one(&f.app)
        .await
        .unwrap(),
        point.cut_ms - HTML_CLOCK_MARGIN_MS
    );
    let old = health.begin(epoch).await.unwrap();
    let newer = health.begin(epoch).await.unwrap();
    assert_eq!(
        health.finish(old, None, 2, 1).await,
        Err(HtmlHealthError::Expired)
    );
    sqlx::query("UPDATE ril_flow_health_epochs SET confirmed_ms=$2-10000,lease_until_ms=$2-1 WHERE epoch=$1").bind(epoch).bind(newer.cut_ms).execute(&f.app).await.unwrap();
    assert_eq!(
        health.begin(epoch).await.unwrap_err(),
        HtmlHealthError::Expired
    );
    assert_eq!(
        health.renew(newer).await.unwrap_err(),
        HtmlHealthError::Expired
    );
    assert_eq!(
        health.finish(newer, None, 2, 1).await,
        Err(HtmlHealthError::Expired)
    );
    health
        .retire(
            epoch,
            2,
            1,
            &HtmlIngressStopped::after_http_stopped_and_drained(),
        )
        .await
        .unwrap();
    assert_eq!(
        health.begin(epoch).await.unwrap_err(),
        HtmlHealthError::Expired
    );
    let fresh = PgHtmlHealth::new(f.app.clone());
    fresh.start_epoch().await.unwrap();
    let row = sqlx::query("SELECT retired,confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1")
        .bind(epoch)
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert!(row.get::<bool, _>("retired"));
    assert_eq!(row.get::<i64, _>("confirmed_ms"), newer.cut_ms - 10000);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires fresh isolated PostgreSQL with HTML health migration"]
async fn html_done_flow_batch_needs_its_receipt_but_a_technical_batch_does_not() {
    let f = Fixture::new().await;
    let health = PgHtmlHealth::new(f.app.clone());
    let ril = PgRilRollups::new(f.app.clone());
    let (epoch, clock) = health.start_epoch().await.unwrap();
    sqlx::query("UPDATE ril_flow_health_epochs SET started_ms=$2-10000,confirmed_ms=$2-10000 WHERE epoch=$1")
        .bind(epoch).bind(clock).execute(&f.app).await.unwrap();
    let point = health.begin(epoch).await.unwrap();
    let mut flow = RollupDelta::default();
    flow.record_observed_flow(page(), Some(f.hour().await));
    let flow_id = Uuid::new_v4().to_string();
    ril.enqueue(&flow_id, &flow).await.unwrap();
    // The isolated LIKE fixture deliberately lacks production triggers. This
    // tests the owner's acknowledgement port against a mixed/invalid old write.
    sqlx::query("UPDATE ril_flush_batches SET status='done' WHERE batch_id=$1")
        .bind(&flow_id)
        .execute(&f.app)
        .await
        .unwrap();
    assert!(
        !health.finish(point, Some(&flow_id), 0, 0).await.unwrap(),
        "done without the FLOW receipt is not evidence of observed-row persistence"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT confirmed_ms FROM ril_flow_health_epochs WHERE epoch=$1"
        )
        .bind(epoch)
        .fetch_one(&f.app)
        .await
        .unwrap(),
        clock - 10000
    );
    sqlx::query("UPDATE ril_flush_batches SET status='pending' WHERE batch_id=$1")
        .bind(&flow_id)
        .execute(&f.app)
        .await
        .unwrap();
    let claim = ril.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id, flow_id);
    assert!(ril.materialize(&claim).await.unwrap());
    assert!(health.finish(point, Some(&flow_id), 0, 0).await.unwrap());
    let technical_point = health.begin(epoch).await.unwrap();
    let mut technical = RollupDelta::default();
    technical.record(event(), None);
    let technical_id = Uuid::new_v4().to_string();
    ril.enqueue(&technical_id, &technical).await.unwrap();
    let claim = ril.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id, technical_id);
    assert!(ril.materialize(&claim).await.unwrap());
    let has_receipt: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM ril_flow_observed_receipts WHERE batch_id=$1)",
    )
    .bind(&technical_id)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert!(
        !has_receipt,
        "a purely technical RIL batch legitimately has no FLOW receipt"
    );
    assert!(health
        .finish(technical_point, Some(&technical_id), 0, 0)
        .await
        .unwrap());
    let discard_point = health.begin(epoch).await.unwrap();
    let mut discarded = RollupDelta::default();
    discarded.record_observed_flow(page(), None);
    let discarded_id = Uuid::new_v4().to_string();
    ril.enqueue(&discarded_id, &discarded).await.unwrap();
    sqlx::query("UPDATE ril_flush_batches SET status='done' WHERE batch_id=$1")
        .bind(&discarded_id)
        .execute(&f.app)
        .await
        .unwrap();
    assert!(
        !health
            .finish(discard_point, Some(&discarded_id), 0, 0)
            .await
            .unwrap(),
        "discard-only FLOW batches also need the immutable receipt"
    );
    f.finish().await;
}
