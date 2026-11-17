//! DROP-07d disposable PG18+MinIO manual observer: never GC or prod.
#![cfg(feature = "s3-free")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_books::repositories::{free_inventory_cycles::InventoryNamespace, PgCatalog};
use tor_web::free_pack_worker::inventory_operator::{OperatorObservedScanner, OperatorScanStep};

#[tokio::test]
#[ignore = "requires isolated disposable PG18/tor_ci and loopback MinIO"]
async fn drop_07d_one_observed_page_commits_unknown_counts_without_gc() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let endpoint = env::var("TOR_TEST_S3_ENDPOINT").unwrap();
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never run against a cloud provider"
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "drop07d-loopback",
        ))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let s3 = Client::from_conf(config);
    const BUCKET: &str = "tor-private-drop07d-ci";
    s3.create_bucket().bucket(BUCKET).send().await.unwrap();
    for key in [
        "private/free-chapters/70007/70001.md",
        "private/free-chapters/70007/70002.md",
    ] {
        s3.put_object()
            .bucket(BUCKET)
            .key(key)
            .body(ByteStream::from(b"synthetic byte-only evidence".to_vec()))
            .send()
            .await
            .unwrap();
    }
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let operator = OperatorObservedScanner::new(&catalog, s3.clone(), BUCKET.to_owned()).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let response = operator
        .run_one(InventoryNamespace::Single, now)
        .await
        .unwrap();
    let cycle_id = match response {
        OperatorScanStep::Completed {
            cycle_id,
            page_index,
            counters,
        } => {
            assert_eq!(page_index, 1);
            assert_eq!(counters.inspected, 2);
            assert_eq!(counters.registered, 0);
            assert_eq!(counters.protected_pending, 0);
            assert_eq!(counters.unknown, 2);
            cycle_id
        }
        other => panic!("expected one completed bounded operator-only page: {other:?}"),
    };
    let counted: (String, i64, i64, i64) = sqlx::query_as(
        "SELECT status,page_index,inspected_count,unknown_count
         FROM free_storage_inventory_cycles WHERE id=$1",
    )
    .bind(cycle_id)
    .fetch_one(catalog.pool())
    .await
    .unwrap();
    assert_eq!(counted, ("completed".to_owned(), 1, 2, 2));
    assert!(
        s3.get_object()
            .bucket(BUCKET)
            .key("private/free-chapters/70007/70001.md")
            .send()
            .await
            .is_ok(),
        "operator evidence must preserve source bytes"
    );
    println!("DROP07D_PG_MINIO_OPERATOR_PASS: single bounded LIST+PG classification, fenced checkpoint, unknown never GC");
}
