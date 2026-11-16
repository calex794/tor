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
    let unavailable_catalog = PgCatalog::connect(&url).await.unwrap();
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
        &path,
        Some(&buyer_cookie),
        Some(&csrf),
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
            &url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures
                 WHERE purchase_id={purchase_id} AND user_id={buyer}"
            ),
        ),
        0,
        "no first exposure may be recorded by a denied grant"
    );
}
