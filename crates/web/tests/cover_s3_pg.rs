//! OBS-09: REAL private S3 (MinIO) + REAL PostgreSQL + asynchronous worker
//! + public HTTP, all on a disposable local CI instance. Never run on a VPS.
#![cfg(feature = "s3-covers")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::Arc, time::Duration};
use tor_books::repositories::{
    cover_slots::{PgCoverSlots, PublicCoverKind},
    PgCatalog,
};
use tor_ingest::{
    repositories::{cover_jobs::PgCoverJobs, s3::S3CoverStore},
    services::{
        cover_jobs::{CoverJobs, CoverObjectStore},
        image_pipeline::RECIPE_VERSION,
    },
};
use tor_reading::repositories::PgReader;
use tor_web::cover_worker::{CoverRunner, CoverRunnerMonitor};
use tower::ServiceExt;

const BUCKET: &str = "tor-private-obs09-ci";
const BOOK_ID: i64 = 950;
const SOURCE_KEY: &str = "staging/covers/sources/obs09-owner-950.png";

fn source_png() -> Vec<u8> {
    let image = RgbImage::from_fn(120, 190, |x, y| Rgb([41, x as u8, y as u8]));
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut output, ImageFormat::Png)
        .expect("small CI PNG");
    output.into_inner()
}

fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

async fn request(app: &axum::Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .expect("local router must return HTTP response")
}

fn local_client() -> Client {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("loopback S3 endpoint");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "OBS-09 must never contact an external provider"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("CI-only username");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("CI-only password");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(access, secret, None, None, "obs09-ci"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(2))
        .build();
    Client::from_conf(config)
}

#[tokio::test]
#[ignore = "requires isolated PG18, seeded owner/book 950 and private loopback MinIO"]
async fn obs09_private_s3_postgres_worker_http_and_tamper_denial() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(
        url.contains("/tor_ci"),
        "OBS-09 must run only against the disposable tor_ci database"
    );
    let catalog = PgCatalog::connect(&url).await.expect("CI database connection");
    let slots = PgCoverSlots::new(catalog.pool());
    let jobs = PgCoverJobs::new(catalog.pool());
    let client = local_client();
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("create unlisted disposable private bucket");
    let store = S3CoverStore::new(client.clone(), BUCKET.to_owned()).unwrap();

    let png = source_png();
    let hash = digest(&png);
    store
        .put_source_verified(SOURCE_KEY, &png, "image/png", &hash)
        .await
        .expect("real signed PUT + HEAD + bounded digest-checked GET");
    let source = jobs
        .register_verified_source(
            BOOK_ID,
            BOOK_ID,
            SOURCE_KEY,
            "image/png",
            png.len() as i64,
            &hash,
        )
        .await
        .expect("trusted PostgreSQL source receipt");
    let job = jobs
        .enqueue(source, BOOK_ID, BOOK_ID, 0, RECIPE_VERSION)
        .await
        .expect("durable fenced cover job");

    let app = tor_web::router_with_postgres_reading_covers(
        catalog.clone(),
        PgReader::new(catalog.pool()),
        Arc::new(store.clone()),
    );
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::NOT_FOUND,
        "an S3 source is not automatically a publicly attached cover"
    );

    // Only the real background runner may claim/verify/store/attach both
    // WebP variants. The test never calls CoverWorker::run_one manually.
    let monitor = CoverRunnerMonitor::default();
    let handle = CoverRunner::new(catalog.clone(), Arc::new(store.clone()), monitor.clone())
        .spawn(Duration::from_millis(40), Duration::from_secs(90));
    let completion = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if monitor.snapshot().attached >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    })
    .await;
    handle.abort();
    let _ = handle.await;
    completion.expect("background runner must attach a cover from real S3 + PG");
    let snapshot = monitor.snapshot();
    assert_eq!(snapshot.attached, 1);
    assert_eq!(snapshot.timeouts, 0);
    assert_eq!(snapshot.errors, 0);
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), Some((job, 1)));
    assert_eq!(jobs.job_status(job).await.unwrap().as_deref(), Some("done"));

    let stored_variants = jobs.existing_variants(job).await.unwrap();
    assert_eq!(stored_variants.len(), 2);
    assert_eq!(stored_variants[0].name, "card");
    assert_eq!(stored_variants[1].name, "main");
    for variant in &stored_variants {
        assert!(store
            .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
            .await
            .expect("signed private S3 HEAD"));
        let image = store
            .get_bounded(&variant.storage_key, variant.size_bytes as usize)
            .await
            .expect("signed bounded S3 GET");
        assert!(image.starts_with(b"RIFF"));
        assert_eq!(image.get(8..12), Some(&b"WEBP"[..]));
        assert_eq!(digest(&image), variant.sha256);
    }

    let card = slots
        .public_variant(BOOK_ID, PublicCoverKind::Card)
        .await
        .unwrap()
        .expect("published verified cover");
    assert_eq!(card.revision, 1);
    let card_original = store
        .get_bounded(&card.storage_key, card.size_bytes as usize)
        .await
        .unwrap();
    let response = request(&app, "/covers/950/card.webp").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/webp");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(response.headers().get("location").is_none());
    let public_bytes = to_bytes(response.into_body(), 8 * 1024 * 1024).await.unwrap();
    assert_eq!(digest(&public_bytes), card.sha256);
    assert_eq!(public_bytes.as_ref(), card_original.as_slice());
    assert_eq!(
        request(&app, "/covers/950/main.webp").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/covers/951/card.webp").await.status(),
        StatusCode::NOT_FOUND
    );

    // A malicious provider writer can forge HEAD metadata. The public HTTP
    // adapter still checks GET bytes and refuses to leak corrupted images.
    let mut damaged = card_original.clone();
    *damaged.last_mut().unwrap() ^= 0x7f;
    let claimed_hash: String = card.sha256.iter().map(|byte| format!("{byte:02x}")).collect();
    client
        .put_object()
        .bucket(BUCKET)
        .key(&card.storage_key)
        .content_type("image/webp")
        .metadata("sha256", claimed_hash)
        .body(ByteStream::from(damaged))
        .send()
        .await
        .unwrap();
    assert!(store
        .exists_verified(&card.storage_key, &card.sha256, card.size_bytes)
        .await
        .unwrap());
    let corrupted = request(&app, "/covers/950/card.webp").await;
    assert_eq!(corrupted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(to_bytes(corrupted.into_body(), 1024).await.unwrap().is_empty());

    store
        .put_verified(&card.storage_key, &card_original)
        .await
        .expect("signed repair of a corrupted private S3 object");
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        slots.current_slot(BOOK_ID).await.unwrap(),
        Some((job, 1)),
        "recovering a stored object must not attach another revision"
    );
    catalog.pool().close().await;
}
