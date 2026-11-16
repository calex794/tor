//! S005/R1: PostgreSQL outages never produce paid reader bearer grants.
use super::*;

pub(super) async fn verify(
    url: &str,
    path: &str,
    buyer_cookie: &str,
    csrf: &str,
    purchase_id: i64,
    buyer: i64,
    fake: &Arc<FakeG>,
) {
    // S005/R1: a buyer with a valid cookie and paid entitlement must not
    // receive a signed URL, DEK, or first-exposure fact during PG outage.
    // Only this test-local router pool is disconnected, not the shared CI PG.
    let unavailable_catalog = PgCatalog::connect(url).await.unwrap();
    let unavailable_pool = unavailable_catalog.pool();
    let unavailable_app = tor_web::router_with_postgres_reading_auth_paid_grants(
        unavailable_catalog,
        PgReader::new(unavailable_pool.clone()),
        PgAccounts::from_pool(unavailable_pool.clone()),
        false,
        ORIGIN.to_owned(),
        fake.clone(),
        None,
    );
    unavailable_pool.close().await;
    let outage_response = send(
        &unavailable_app,
        path,
        Some(buyer_cookie),
        Some(csrf),
        Some(ORIGIN),
        &json!({"purchase_id": purchase_id.to_string()}).to_string(),
    )
    .await;
    assert_eq!(
        outage_response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "PG outage must not look like a successful paid grant"
    );
    assert_eq!(outage_response.headers()[header::CACHE_CONTROL], "no-store");
    let outage_body = to_bytes(outage_response.into_body(), 8192).await.unwrap();
    assert!(
        !has_private_reader_material(&outage_body),
        "failed PG cannot expose paid text or signing metadata"
    );
    assert_eq!(fake.signed.load(Ordering::SeqCst), 0);
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures
                 WHERE purchase_id={purchase_id} AND user_id={buyer}"
            ),
        ),
        0,
        "no first exposure may be recorded by a denied grant"
    );

    // S005/R1: a live entitlement and healthy PG must not circumvent an
    // unavailable private G signer. Reject with 503 before the browser can
    // see any bearer URL/DEK, and do not log a successful first exposure.
    let available_catalog = PgCatalog::connect(url).await.unwrap();
    let available_pool = available_catalog.pool().clone();
    let available_app = tor_web::router_with_postgres_reading_auth_paid_grants(
        available_catalog,
        PgReader::new(available_pool.clone()),
        PgAccounts::from_pool(available_pool.clone()),
        false,
        ORIGIN.to_owned(),
        fake.clone(),
        None,
    );
    assert_eq!(fake.signed.load(Ordering::SeqCst), 0);
    fake.fail_next.store(1, Ordering::SeqCst);
    let g_outage = send(
        &available_app,
        path,
        Some(buyer_cookie),
        Some(csrf),
        Some(ORIGIN),
        &json!({"purchase_id": purchase_id.to_string()}).to_string(),
    )
    .await;
    assert_eq!(
        g_outage.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a G signing error must fail closed instead of serving paid data"
    );
    assert_eq!(g_outage.headers()[header::CACHE_CONTROL], "no-store");
    let denied_bytes = to_bytes(g_outage.into_body(), 8192).await.unwrap();
    assert!(
        !has_private_reader_material(&denied_bytes),
        "G outage may never disclose chapter ciphertext URL or scoped DEK"
    );
    assert_eq!(fake.fail_next.load(Ordering::SeqCst), 0);
    assert_eq!(fake.signed.load(Ordering::SeqCst), 1);
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures
                 WHERE purchase_id={purchase_id} AND user_id={buyer}"
            ),
        ),
        0,
        "a failed G grant is not a completed first read"
    );
    // S005/R1: G can finish signing just as PostgreSQL becomes unavailable.
    // The second D authorization must refuse to return that already-created
    // bearer URL, even though the first D check and G signing succeeded.
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    *fake.pause_next.lock().unwrap() = Some((entered_tx, release_rx));
    let race_app = available_app.clone();
    let race_path = path.to_owned();
    let race_cookie = buyer_cookie.to_owned();
    let race_csrf = csrf.to_owned();
    let in_flight = tokio::spawn(async move {
        send(
            &race_app,
            &race_path,
            Some(&race_cookie),
            Some(&race_csrf),
            Some(ORIGIN),
            &json!({"purchase_id": purchase_id.to_string()}).to_string(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), entered_rx)
        .await
        .expect("G signing should start with valid D entitlement")
        .expect("G reached test barrier");
    // This pool belongs only to the test router, not the main CI database.
    available_pool.close().await;
    release_tx.send(()).expect("release simulated G signing");
    let final_pg_outage = tokio::time::timeout(Duration::from_secs(10), in_flight)
        .await
        .expect("grant must finish after PG outage")
        .expect("join grant task");
    assert_eq!(final_pg_outage.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(final_pg_outage.headers()[header::CACHE_CONTROL], "no-store");
    let final_body = to_bytes(final_pg_outage.into_body(), 8192).await.unwrap();
    assert!(
        !has_private_reader_material(&final_body),
        "a signed G URL must not leak when the final entitlement check fails"
    );
    assert_eq!(fake.signed.load(Ordering::SeqCst), 2);
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures
                 WHERE purchase_id={purchase_id} AND user_id={buyer}"
            ),
        ),
        0,
        "failed final authorization cannot mark first read"
    );
    // Existing success path on the unaffected primary router must recover
    // for the same purchased V1 after both independent injected outages.
    fake.signed.store(0, Ordering::SeqCst);
}
