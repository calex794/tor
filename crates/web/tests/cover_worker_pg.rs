//! End-to-end internal cover worker: real PostgreSQL, deterministic S3 fake.
//! Real S3/HTTP upload/browser availability requires independent provider evidence.
use async_trait::async_trait;
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

    catalog.pool().close().await;
}
