//! DROP-07d disposable PG18+MinIO manual observer: never GC or prod.
#![cfg(feature = "s3-free")]

#[path = "free_inventory_operator_pg_minio/drop_quarantine_drill.rs"]
mod drop_quarantine_drill;

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use sha2::{Digest, Sha256};
use std::{
    env,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_books::repositories::{free_inventory_cycles::InventoryNamespace, PgCatalog};
use tor_web::free_pack_worker::inventory_operator::{OperatorObservedScanner, OperatorScanStep};

#[tokio::test]
#[ignore = "requires isolated disposable PG18/tor_ci and loopback MinIO"]
async fn drop_07d_observed_pages_resume_and_commit_unknown_counts_without_gc() {
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
    for version in 70001..=70065 {
        s3.put_object()
            .bucket(BUCKET)
            .key(format!("private/free-chapters/70007/{version}.md"))
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
    let first = operator
        .run_one(InventoryNamespace::Single, now)
        .await
        .unwrap();
    let cycle_id = match first {
        OperatorScanStep::Advanced {
            cycle_id,
            page_index,
            counters,
        } => {
            assert_eq!(page_index, 1);
            assert_eq!(counters.inspected, 64);
            assert_eq!(counters.registered, 0);
            assert_eq!(counters.protected_pending, 0);
            assert_eq!(counters.unknown, 64);
            cycle_id
        }
        other => panic!("expected the first bounded operator page: {other:?}"),
    };
    // A fresh coordinator resumes the persisted token immediately.
    let resumed = OperatorObservedScanner::new(&catalog, s3.clone(), BUCKET.to_owned()).unwrap();
    match resumed
        .run_one(InventoryNamespace::Single, now)
        .await
        .unwrap()
    {
        OperatorScanStep::Completed {
            cycle_id: resumed_id,
            page_index,
            counters,
        } => {
            assert_eq!(resumed_id, cycle_id);
            assert_eq!(page_index, 2);
            assert_eq!(counters.inspected, 1);
            assert_eq!(counters.unknown, 1);
        }
        other => panic!("expected completion of the same cycle: {other:?}"),
    }
    assert_eq!(
        fixture_sql(
            &url,
            &format!(
                "SELECT status,page_index,inspected_count,unknown_count
                 FROM free_storage_inventory_cycles WHERE id={cycle_id}"
            ),
        ),
        "completed|2|65|65"
    );
    assert!(
        s3.get_object()
            .bucket(BUCKET)
            .key("private/free-chapters/70007/70001.md")
            .send()
            .await
            .is_ok(),
        "operator evidence must preserve source bytes"
    );
    // Stage one exact archive in MinIO and deliberately omit the registered
    // manifest. The expected scan must account for both PostgreSQL receipts.
    const PACK_KEY: &str = "private/free-packs/ci/drop07e-expected-immutable.zst";
    const MANIFEST_KEY: &str = "private/free-packs/ci/drop07e-expected-immutable.manifest.json";
    let body = b"synthetic bytes for a registered PACK receipt";
    let digest = hex::encode(Sha256::digest(body));
    fixture_sql(
        &url,
        &format!(
            "INSERT INTO free_storage_packs
             (bucket,object_key,pack_sha256,manifest_key,manifest_sha256,
              pack_bytes,manifest_bytes,seek_table_start)
             VALUES ('{BUCKET}','{PACK_KEY}',decode('{digest}','hex'),'{MANIFEST_KEY}',
                     decode(repeat('11',32),'hex'),{},1,1)",
            body.len(),
        ),
    );
    s3.put_object()
        .bucket(BUCKET)
        .key(PACK_KEY)
        .body(ByteStream::from(body.to_vec()))
        .send()
        .await
        .unwrap();
    let expected_id = match resumed
        .run_expected_one(InventoryNamespace::Pack)
        .await
        .unwrap()
    {
        OperatorScanStep::Completed {
            cycle_id,
            page_index,
            counters,
        } => {
            assert_eq!(page_index, 1);
            assert_eq!(counters.inspected, 2);
            assert_eq!(counters.verified, 1);
            assert_eq!(counters.unverifiable, 1);
            cycle_id
        }
        other => panic!("expected one completed PACK receipt page: {other:?}"),
    };
    assert_eq!(
        fixture_sql(
            &url,
            &format!(
                "SELECT direction,namespace,status,inspected_count,matched_count,unverifiable_count
                 FROM free_storage_inventory_cycles WHERE id={expected_id}"
            ),
        ),
        "pg_to_s3|pack|completed|2|1|1"
    );
    println!("DROP07D_PG_MINIO_OPERATOR_PASS: bounded LIST+PG classification across two pages, fenced checkpoint, unknown never GC");
}

fn fixture_sql(url: &str, query: &str) -> String {
    let output = Command::new("psql")
        .args([
            "-XAt",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "-c",
            query,
        ])
        .output()
        .expect("psql client for the disposable database");
    assert!(
        output.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
