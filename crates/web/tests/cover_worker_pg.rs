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
use tor_web::cover_worker::PgBookCoverBridge;
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
    assert_eq!(worker.run_one().await.unwrap(), WorkerResult::Attached);
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

    // B-owned PostgreSQL tests independently verify hold/draft/delete revocation.
    // The web integration intentionally has no SQLx dependency.
    catalog.pool().close().await;
}
