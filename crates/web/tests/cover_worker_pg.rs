//! End-to-end internal cover worker: real PostgreSQL, deterministic S3 fake.
//! Real S3/HTTP upload/browser availability requires independent provider evidence.
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
    time::Duration,
};
use tor_books::repositories::{
    cover_slots::{PgCoverSlots, PublicCoverKind},
    PgCatalog,
};
use tor_ingest::{
    domain::cover::CoverError,
    repositories::cover_jobs::PgCoverJobs,
    services::{
        cover_jobs::{CoverJobs, CoverObjectStore},
        cover_worker::{CoverWorker, WorkerResult},
        image_pipeline::RECIPE_VERSION,
    },
};
use tor_web::cover_worker::{CoverRunner, CoverRunnerMonitor, PgBookCoverBridge};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct MemoryS3 {
    inner: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}
impl MemoryS3 {
    fn add(&self, key: &str, data: Vec<u8>) {
        self.inner.lock().unwrap().insert(key.into(), data);
    }
}
fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}
#[async_trait]
impl CoverObjectStore for MemoryS3 {
    async fn get_bounded(&self, key: &str, limit: usize) -> Result<Vec<u8>, CoverError> {
        let data = self
            .inner
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(CoverError::Unavailable)?;
        if data.len() > limit {
            return Err(CoverError::Invalid);
        }
        Ok(data)
    }
    async fn put_verified(&self, key: &str, bytes: &[u8]) -> Result<(), CoverError> {
        if !key.starts_with("staging/covers/variants/") {
            return Err(CoverError::Forbidden);
        }
        self.add(key, bytes.to_vec());
        Ok(())
    }
    async fn exists_verified(
        &self,
        key: &str,
        hash: &[u8; 32],
        size: i64,
    ) -> Result<bool, CoverError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|data| data.len() as i64 == size && digest(data) == *hash))
    }
}
fn png(seed: u8) -> Vec<u8> {
    let img = RgbImage::from_fn(120, 190, |x, y| Rgb([seed, x as u8, y as u8]));
    let mut writer = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img)
        .write_to(&mut writer, ImageFormat::Png)
        .unwrap();
    writer.into_inner()
}

async fn fetch(app: &axum::Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires seeded TOR PG18 and a deterministic fake S3 store"]
async fn g_09_verified_variants_and_b_atomic_slot_replacement() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let jobs = PgCoverJobs::new(pool.clone());
    let slots = PgCoverSlots::new(pool);
    let bridge = PgBookCoverBridge::new(slots.clone());
    let store = MemoryS3::default();
    let worker = CoverWorker {
        jobs: &jobs,
        storage: &store,
        books: &bridge,
    };

    // First generation: one verified source, two real WebP variants, one B slot.
    let bytes = png(42);
    let key = "staging/covers/ci-book-901-source-1.png";
    store.add(key, bytes.clone());
    let source = jobs
        .register_verified_source(
            901,
            901,
            key,
            "image/png",
            bytes.len() as i64,
            &digest(&bytes),
        )
        .await
        .unwrap();
    let job = jobs
        .enqueue(source, 901, 901, 0, RECIPE_VERSION)
        .await
        .unwrap();
    // OBS-07: prove the actual scheduled loop autonomously takes a PG job,
    // produces two verified variants and attaches it. No manual run_one().
    let monitor = CoverRunnerMonitor::default();
    let handle = CoverRunner::new(catalog.clone(), Arc::new(store.clone()), monitor.clone())
        .spawn(Duration::from_millis(25), Duration::from_secs(90));
    let outcome = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if monitor.snapshot().attached >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    handle.abort();
    outcome.expect("automated cover runner must finish the first PG/S3-fake job");
    let local = monitor.snapshot();
    assert_eq!(local.attached, 1);
    assert_eq!(local.errors, 0);
    assert_eq!(local.timeouts, 0);
    assert_eq!(slots.current_slot(901).await.unwrap(), Some((job, 1)));
    assert_eq!(jobs.job_status(job).await.unwrap().as_deref(), Some("done"));
    let card = slots
        .public_variant(901, PublicCoverKind::Card)
        .await
        .unwrap()
        .expect("a verified public card");
    assert_eq!(card.revision, 1);
    let object = store
        .get_bounded(&card.storage_key, 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(object.len() as i64, card.size_bytes);
    assert_eq!(digest(&object), card.sha256);
    let app = tor_web::router_with_postgres_reading_covers(
        catalog.clone(),
        tor_reading::repositories::PgReader::new(catalog.pool()),
        Arc::new(store.clone()),
    );
    let response = fetch(&app, "/covers/901/card.webp").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/webp");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(response.headers().get("location").is_none());
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(digest(&body), card.sha256);
    assert_eq!(
        fetch(&app, "/covers/901/main.webp").await.status(),
        StatusCode::OK
    );
    for route in [
        "/covers/901/thumb.webp",
        "/covers/902/card.webp",
        "/covers/903/card.webp",
        "/covers/0/card.webp",
        "/covers/not-an-id/card.webp",
        "/covers/999999999999999999999999999999/card.webp",
    ] {
        let response = fetch(&app, route).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    let book = fetch(&app, "/books/901").await;
    assert_eq!(book.status(), StatusCode::OK);
    let html = to_bytes(book.into_body(), 256 * 1024).await.unwrap();
    let html = std::str::from_utf8(&html).unwrap();
    assert!(html.contains("src=\"/covers/901/main.webp\""));
    assert!(!html.contains(&card.storage_key));
    let catalog_page = fetch(&app, "/catalog?q=CI%20owned%20cover%20book").await;
    assert_eq!(catalog_page.status(), StatusCode::OK);
    let body = to_bytes(catalog_page.into_body(), 256 * 1024)
        .await
        .unwrap();
    assert!(std::str::from_utf8(&body)
        .unwrap()
        .contains("src=\"/covers/901/card.webp\""));

    // Damaged private-object bytes fail closed without exposing partial data.
    store.add(&card.storage_key, vec![0x41; object.len()]);
    let bad = fetch(&app, "/covers/901/card.webp").await;
    assert_eq!(bad.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(to_bytes(bad.into_body(), 1024).await.unwrap().is_empty());
    store.add(&card.storage_key, object);
    assert!(slots
        .public_variant(901, PublicCoverKind::Main)
        .await
        .unwrap()
        .is_some());
    assert!(slots
        .public_variant(0, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());
    let variants = jobs.existing_variants(job).await.unwrap();
    assert_eq!(variants.len(), 2);
    assert_eq!(variants[0].name, "card");
    assert_eq!(variants[1].name, "main");
    for v in &variants {
        assert!(store
            .exists_verified(&v.storage_key, &v.sha256, v.size_bytes)
            .await
            .unwrap());
    }
    // Idempotent HTTP command replay cannot create another paid or public cover job.
    assert_eq!(
        jobs.enqueue(source, 901, 901, 0, RECIPE_VERSION)
            .await
            .unwrap(),
        job
    );
    assert_eq!(slots.current_slot(901).await.unwrap(), Some((job, 1)));

    // Replace with new verified source, CAS against revision 1.
    let changed = png(99);
    let new_key = "staging/covers/ci-book-901-source-2.png";
    store.add(new_key, changed.clone());
    let id = jobs
        .register_verified_source(
            901,
            901,
            new_key,
            "image/png",
            changed.len() as i64,
            &digest(&changed),
        )
        .await
        .unwrap();
    let second = jobs.enqueue(id, 901, 901, 1, RECIPE_VERSION).await.unwrap();
    assert_eq!(worker.run_one().await.unwrap(), WorkerResult::Attached);
    assert_eq!(slots.current_slot(901).await.unwrap(), Some((second, 2)));
    assert_ne!(second, job);
    let current = slots
        .public_variant(901, PublicCoverKind::Card)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, 2);
    assert_ne!(current.sha256, card.sha256);
    let fresh = fetch(&app, "/covers/901/card.webp").await;
    assert_eq!(fresh.status(), StatusCode::OK);
    let bytes = to_bytes(fresh.into_body(), 8 * 1024 * 1024).await.unwrap();
    assert_eq!(digest(&bytes), current.sha256);
    assert_eq!(
        jobs.existing_variants(job).await.unwrap().len(),
        2,
        "old generation retained until separate GC decision"
    );

    // Two jobs race for the same empty slot; only one may publish.
    let left = png(21);
    let right = png(22);
    for (n, bytes) in [left, right].into_iter().enumerate() {
        let key = format!("staging/covers/ci-book-902-race-{n}.png");
        store.add(&key, bytes.clone());
        let source = jobs
            .register_verified_source(
                902,
                902,
                &key,
                "image/png",
                bytes.len() as i64,
                &digest(&bytes),
            )
            .await
            .unwrap();
        jobs.enqueue(source, 902, 902, 0, RECIPE_VERSION)
            .await
            .unwrap();
    }
    let first = worker.run_one().await.unwrap();
    let latter = worker.run_one().await.unwrap();
    assert_eq!(
        [first, latter],
        [WorkerResult::Attached, WorkerResult::Superseded]
    );
    assert_eq!(slots.current_slot(902).await.unwrap().unwrap().1, 1);

    // Bad source hash is terminal, never attached or exposed to public readers.
    let malicious = png(5);
    let source_key = "staging/covers/ci-book-903-bad-sha.png";
    store.add(source_key, malicious.clone());
    let bad = jobs
        .register_verified_source(
            903,
            903,
            source_key,
            "image/png",
            malicious.len() as i64,
            &[7_u8; 32],
        )
        .await
        .unwrap();
    let bad_job = jobs
        .enqueue(bad, 903, 903, 0, RECIPE_VERSION)
        .await
        .unwrap();
    assert_eq!(
        worker.run_one().await.unwrap(),
        WorkerResult::RetryOrQuarantine
    );
    assert_eq!(
        jobs.job_status(bad_job).await.unwrap().as_deref(),
        Some("dead")
    );
    assert!(slots.current_slot(903).await.unwrap().is_none());
    assert!(slots
        .public_variant(903, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());

    // OBS-08: the private S3 provider disappears after trusted registration.
    // The scheduled runner must keep the job pending, with no public image.
    // Then abort it (simulated process restart), restore the object, and verify
    // a fresh runner resumes from durable PG without duplicate attachment.
    let after_outage_bytes = png(73);
    let outage_key = "staging/covers/ci-book-904-restart-source.png";
    let source = jobs
        .register_verified_source(
            904,
            904,
            outage_key,
            "image/png",
            after_outage_bytes.len() as i64,
            &digest(&after_outage_bytes),
        )
        .await
        .unwrap();
    let interrupted_job = jobs
        .enqueue(source, 904, 904, 0, RECIPE_VERSION)
        .await
        .unwrap();
    assert_eq!(
        fetch(&app, "/covers/904/card.webp").await.status(),
        StatusCode::NOT_FOUND,
        "unattached private source must not be publicly readable"
    );
    let original_monitor = CoverRunnerMonitor::default();
    let original = CoverRunner::new(
        catalog.clone(),
        Arc::new(store.clone()),
        original_monitor.clone(),
    )
    .spawn(Duration::from_millis(25), Duration::from_secs(90));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if original_monitor.snapshot().retried_or_quarantined >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("S3 outage must be reported by the actual background worker");
    original.abort();
    let stopped = original.await.unwrap_err();
    assert!(
        stopped.is_cancelled(),
        "simulated restart must stop the first runner"
    );
    let first = original_monitor.snapshot();
    assert_eq!(first.attached, 0);
    assert!(first.retried_or_quarantined >= 1);
    assert_eq!(first.timeouts, 0);
    assert_eq!(
        jobs.job_status(interrupted_job).await.unwrap().as_deref(),
        Some("pending"),
        "a temporary outage must preserve the durable job for retry"
    );
    assert!(slots.current_slot(904).await.unwrap().is_none());
    assert_eq!(
        fetch(&app, "/covers/904/card.webp").await.status(),
        StatusCode::NOT_FOUND,
        "the outage may not publish an unverified object"
    );

    // Simulate provider recovery and a fresh web process reusing the same PG.
    store.add(outage_key, after_outage_bytes);
    let restarted_monitor = CoverRunnerMonitor::default();
    let restarted = CoverRunner::new(
        catalog.clone(),
        Arc::new(store.clone()),
        restarted_monitor.clone(),
    )
    .spawn(Duration::from_millis(25), Duration::from_secs(90));
    let resumed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if restarted_monitor.snapshot().attached >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await;
    restarted.abort();
    let _ = restarted.await;
    resumed.expect("after S3 recovery, a new process must claim the retained job");
    let second = restarted_monitor.snapshot();
    assert_eq!(second.attached, 1);
    assert_eq!(second.timeouts, 0);
    assert_eq!(
        jobs.job_status(interrupted_job).await.unwrap().as_deref(),
        Some("done")
    );
    assert_eq!(
        slots.current_slot(904).await.unwrap(),
        Some((interrupted_job, 1)),
        "replay after restart must attach the exact job once"
    );
    assert_eq!(
        jobs.existing_variants(interrupted_job).await.unwrap().len(),
        2
    );
    let image = fetch(&app, "/covers/904/card.webp").await;
    assert_eq!(image.status(), StatusCode::OK);
    assert_eq!(image.headers()["content-type"], "image/webp");
    assert_eq!(image.headers()["cache-control"], "no-store");

    // B-owned PostgreSQL tests independently verify hold/draft/delete revocation.
    // The web integration intentionally has no SQLx dependency.
    catalog.pool().close().await;
}
