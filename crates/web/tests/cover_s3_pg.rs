//! OBS-09: REAL private S3 (MinIO) + REAL PostgreSQL + asynchronous worker
//! + public HTTP, all on a disposable local CI instance. Never run on a VPS.
#![cfg(feature = "s3-covers")]

use async_trait::async_trait;
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
    cover_publications::PgPublicCoverPublications,
    cover_purge_queue::PgCdnPurgeQueue,
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
use tor_web::{
    cover_cdn_purge_worker::{CdnPurgeProvider, CdnPurgeWorker},
    cover_delivery::CoverDelivery,
    cover_worker::{CoverRunner, CoverRunnerMonitor},
    yandex_cdn_purge::PurgeState,
};
use tower::ServiceExt;

const BUCKET: &str = "tor-private-obs09-ci";
const BOOK_ID: i64 = 950;
const SOURCE_KEY: &str = "staging/covers/sources/obs09-owner-950.png";

/// No external Cloud calls: exercise durable POST/Get stages against PG18.
struct LocalCdnPurge;

#[async_trait]
impl CdnPurgeProvider for LocalCdnPurge {
    async fn submit(&self, path: &str) -> Result<String, tor_ingest::domain::cover::CoverError> {
        assert!(path.starts_with("/public/covers/950/1/"));
        assert!(path.ends_with(".webp*"));
        Ok("ci-purge".into())
    }

    async fn poll(&self, id: &str) -> Result<PurgeState, tor_ingest::domain::cover::CoverError> {
        assert_eq!(id, "ci-purge");
        Ok(PurgeState::Succeeded)
    }
}

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

// Run lifecycle mutations outside the async PostgreSQL executor. A synchronous
// psql call on its worker thread can deadlock a single-threaded Tokio test.
async fn pg_sql(url: &str, sql: &str) -> String {
    let url = url.to_owned();
    let sql = sql.to_owned();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("psql")
            .args([
                "-X",
                "-q",
                "-t",
                "-A",
                "-v",
                "ON_ERROR_STOP=1",
                "-d",
                &url,
                "-c",
                &sql,
            ])
            .output()
            .expect("disposable CI psql");
        assert!(
            output.status.success(),
            "disposable CI SQL failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF-8 psql output")
            .trim()
            .to_owned()
    })
    .await
    .expect("blocking SQL task")
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
    let catalog = PgCatalog::connect(&url)
        .await
        .expect("CI database connection");
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
    // The publication ledger must reserve each immutable identity BEFORE S3 COPY.
    // Replays never create a second row, and copied state remains idempotent.
    let ledger = PgPublicCoverPublications::new(catalog.pool());
    let mut publication_ids = Vec::new();
    for kind in [PublicCoverKind::Card, PublicCoverKind::Main] {
        let reserved = ledger
            .reserve_current(BOOK_ID, kind)
            .await
            .unwrap()
            .expect("current visible cover reserves one immutable row");
        let again = ledger
            .reserve_current(BOOK_ID, kind)
            .await
            .unwrap()
            .expect("repeated reservation returns same row");
        assert_eq!(reserved.id, again.id);
        assert_eq!(reserved.job_id, job);
        assert_eq!(reserved.slot_revision, 1);
        assert!(ledger.confirm_copy(reserved.id).await.unwrap());
        assert!(ledger.confirm_copy(reserved.id).await.unwrap());
        publication_ids.push(reserved.id);
    }
    // The CDN bucket is a separate destination: never publish source PNGs or
    // use the private cover bucket as a publicly-readable bucket.
    const PUBLIC_BUCKET: &str = "tor-public-cover-cdn-ci";
    client
        .create_bucket()
        .bucket(PUBLIC_BUCKET)
        .send()
        .await
        .expect("create disposable public-target bucket");
    let publisher = S3CoverStore::new(client.clone(), BUCKET.to_owned())
        .unwrap()
        .with_public_cdn(PUBLIC_BUCKET.to_owned(), "https://cdn.example".to_owned())
        .unwrap();
    // An S3 adapter with a configured CDN is not enough for permission to
    // publish: the durable B-owned publication ledger is mandatory.
    let unregistered = CoverDelivery::new(slots.clone(), Arc::new(publisher.clone()));
    assert!(matches!(
        unregistered
            .publish_cdn_variant(BOOK_ID as u64, PublicCoverKind::Card)
            .await,
        Err(tor_ingest::domain::cover::CoverError::Forbidden)
    ));
    let direct = CoverDelivery::new(slots.clone(), Arc::new(publisher.clone()))
        .with_publication_registry(ledger.clone());
    let mut public_urls = Vec::new();
    for variant in [PublicCoverKind::Card, PublicCoverKind::Main] {
        let url = direct
            .publish_cdn_variant(BOOK_ID as u64, variant)
            .await
            .unwrap()
            .expect("only an attached public variant can be published");
        assert!(url.starts_with("https://cdn.example/public/covers/950/1/"));
        assert!(url.ends_with(".webp"));
        let key = url.strip_prefix("https://cdn.example/").unwrap();
        public_urls.push((variant, url.clone()));
        let head = client
            .head_object()
            .bucket(PUBLIC_BUCKET)
            .key(key)
            .send()
            .await
            .unwrap();
        assert_eq!(head.content_type(), Some("image/webp"));
        assert_eq!(
            head.cache_control(),
            Some("public, max-age=31536000, immutable")
        );
    }
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
    let public_bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
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

    // An old same-origin cover URL must stop resolving when its public Book is
    // pending deletion, and both list projection and both variants must agree.
    // Existing private S3 variants stay stored; only public access is revoked.
    let previous_lifecycle = pg_sql(
        &url,
        &format!("SELECT lifecycle_status FROM books WHERE id={BOOK_ID}"),
    )
    .await;
    assert!(
        ["in_progress", "completed", "abandoned", "on_hiatus"]
            .contains(&previous_lifecycle.as_str()),
        "expected a public, non-deleting fixture"
    );
    let initial_cards = slots.public_card_ids(&[BOOK_ID as u64]).await.unwrap();
    assert!(initial_cards.contains(&(BOOK_ID as u64)));
    pg_sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='pending_deletion' WHERE id={BOOK_ID}"),
    )
    .await;
    // An operator-owned lifecycle worker can now remove the precise published
    // copies without touching the private originals. An automatic durable
    // revoke queue and Yandex purge are deliberately not enabled yet.
    for (kind, public_url) in &public_urls {
        let name = match kind {
            PublicCoverKind::Card => "card",
            PublicCoverKind::Main => "main",
        };
        let image = stored_variants
            .iter()
            .find(|item| item.name == name)
            .unwrap();
        for _ in 0..2 {
            let revoked_url = publisher
                .remove_public_cover(BOOK_ID as u64, card.revision, name, &image.sha256)
                .await
                .expect("exact public removal is idempotent");
            assert_eq!(revoked_url, *public_url);
        }
        let key = public_url.strip_prefix("https://cdn.example/").unwrap();
        assert!(client
            .head_object()
            .bucket(PUBLIC_BUCKET)
            .key(key)
            .send()
            .await
            .is_err());
        assert!(store
            .exists_verified(&image.storage_key, &image.sha256, image.size_bytes)
            .await
            .unwrap());
    }
    // Hiding the Book turns copied public keys into durable, exclusively
    // leased deletion jobs. A stale holder cannot acknowledge another lease.
    let claims = ledger.claim_removals(10).await.unwrap();
    assert_eq!(claims.len(), 2);
    for item in &claims {
        assert!(publication_ids.contains(&item.id));
        assert_eq!(item.lease_epoch, 1);
        assert!(!ledger.origin_deleted(item.id, 0).await.unwrap());
        assert!(ledger
            .origin_deleted(item.id, item.lease_epoch)
            .await
            .unwrap());
        assert!(!ledger
            .origin_deleted(item.id, item.lease_epoch)
            .await
            .unwrap());
        assert!(!ledger.purge_completed(item.id, "wrong").await.unwrap());
    }
    assert!(ledger.claim_removals(10).await.unwrap().is_empty());
    let purge = CdnPurgeWorker::new(
        PgCdnPurgeQueue::new(catalog.pool()),
        Arc::new(LocalCdnPurge),
    );
    let submitted = purge.run_batch().await.unwrap();
    assert_eq!(submitted.claimed, 2);
    assert_eq!(submitted.submitted, 2);
    assert_eq!(submitted.completed, 0);
    // Only an explicit Operation.Get can complete a CDN purge, not POST.
    for id in &publication_ids {
        assert!(!ledger.purge_completed(*id, "other").await.unwrap());
    }
    pg_sql(
        &url,
        "UPDATE public_cover_publications SET retry_after=now()-interval '1 second'
         WHERE state='purge_submitted'",
    )
    .await;
    let polled = purge.run_batch().await.unwrap();
    assert_eq!(polled.claimed, 2);
    assert_eq!(polled.completed, 2);
    assert_eq!(purge.run_batch().await.unwrap().claimed, 0);
    assert!(
        direct
            .publish_cdn_variant(BOOK_ID as u64, PublicCoverKind::Card)
            .await
            .unwrap()
            .is_none(),
        "a hidden book cannot publish further public CDN copies"
    );
    assert!(
        slots
            .public_card_ids(&[BOOK_ID as u64])
            .await
            .unwrap()
            .is_empty(),
        "a deleted Book cannot appear in homepage/catalog cover projections"
    );
    assert!(slots
        .public_variant(BOOK_ID, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());
    for kind in ["card", "main"] {
        assert_eq!(
            request(&app, &format!("/covers/{BOOK_ID}/{kind}.webp"))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "previous cover URL must be revoked"
        );
    }
    pg_sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='{previous_lifecycle}' WHERE id={BOOK_ID}"),
    )
    .await;
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::OK,
        "restoring a public Book reuses the same validated cover"
    );
    assert_eq!(
        slots.current_slot(BOOK_ID).await.unwrap(),
        Some((job, 1)),
        "lifecycle revocation does not publish a new cover revision"
    );

    // A malicious provider writer can forge HEAD metadata. The public HTTP
    // adapter still checks GET bytes and refuses to leak corrupted images.
    let mut damaged = card_original.clone();
    *damaged.last_mut().unwrap() ^= 0x7f;
    let claimed_hash: String = card
        .sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
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
    assert!(to_bytes(corrupted.into_body(), 1024)
        .await
        .unwrap()
        .is_empty());

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
