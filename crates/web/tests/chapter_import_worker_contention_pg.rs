//! P1: actual S023 importer + G-09 image worker + authenticated HTTP all
//! compete with real REC generation, creator finance reads, and authenticated
//! HTTP on one three-connection PostgreSQL pool. Both private object ports are
//! isolated in-memory fakes; image processing and database jobs are real.
//! No external storage, production data, deployments or schema mutations.
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Cursor,
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Barrier, Mutex};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::{
        authoring::new_book,
        chapter_file_import::{fingerprint, parse_text},
    },
};
use tor_commerce::repositories::creator_finance::PgCreatorFinance;
use tor_imports::services::{ChapterImportObjectStore, ChapterImportStoreError as StoreError};
use tor_ingest::{
    domain::cover::CoverError,
    repositories::cover_jobs::PgCoverJobs,
    services::{
        cover_jobs::{CoverJobs, CoverObjectStore},
        image_pipeline::RECIPE_VERSION,
    },
};
use tor_reading::repositories::PgReader;
use tor_recommendations::repositories::PgRecommendations;
use tor_web::{
    chapter_import_worker::{ChapterImportWorker, ChapterImportWorkerResult},
    cover_worker::{CoverRunner, CoverRunnerMonitor},
};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

#[derive(Default, Clone)]
struct MemoryStore(Arc<Mutex<HashMap<String, Vec<u8>>>>);

#[async_trait]
impl ChapterImportObjectStore for MemoryStore {
    async fn put_verified(
        &self,
        key: &str,
        bytes: &[u8],
        sha256: &[u8; 32],
    ) -> Result<(), StoreError> {
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if !key.starts_with("staging/chapters/") || bytes.is_empty() || actual != *sha256 {
            return Err(StoreError::Invalid);
        }
        let mut store = self.0.lock().await;
        if let Some(existing) = store.get(key) {
            return if existing == bytes {
                Ok(())
            } else {
                Err(StoreError::Conflict)
            };
        }
        store.insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    async fn get_verified(
        &self,
        key: &str,
        expected_size: i64,
        sha256: &[u8; 32],
        max_bytes: usize,
    ) -> Result<Vec<u8>, StoreError> {
        let store = self.0.lock().await;
        let bytes = store.get(key).ok_or(StoreError::Unavailable)?;
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if bytes.len() != expected_size as usize || bytes.len() > max_bytes || actual != *sha256 {
            return Err(StoreError::Corrupt);
        }
        Ok(bytes.clone())
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.0.lock().await.remove(key);
        Ok(())
    }
}

/// Covers are staged and verified in a separate fake S3 port; the real
/// G-09 image worker still performs image decoding, WebP encoding and SQL.
#[derive(Clone, Default)]
struct MemoryCoverStore(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl MemoryCoverStore {
    async fn add(&self, key: &str, bytes: Vec<u8>) {
        self.0.lock().await.insert(key.to_owned(), bytes);
    }
}

#[async_trait]
impl CoverObjectStore for MemoryCoverStore {
    async fn get_bounded(&self, key: &str, max_bytes: usize) -> Result<Vec<u8>, CoverError> {
        let store = self.0.lock().await;
        let source = store.get(key).ok_or(CoverError::Unavailable)?;
        if source.len() > max_bytes {
            return Err(CoverError::Invalid);
        }
        Ok(source.clone())
    }

    async fn put_verified(&self, key: &str, bytes: &[u8]) -> Result<(), CoverError> {
        if !key.starts_with("staging/covers/variants/") {
            return Err(CoverError::Forbidden);
        }
        self.add(key, bytes.to_vec()).await;
        Ok(())
    }

    async fn exists_verified(
        &self,
        key: &str,
        hash: &[u8; 32],
        size: i64,
    ) -> Result<bool, CoverError> {
        Ok(self.0.lock().await.get(key).is_some_and(|bytes| {
            bytes.len() as i64 == size && Sha256::digest(bytes).as_slice() == hash
        }))
    }
}

fn cover_png() -> Vec<u8> {
    // Nontrivial pixels: this exercises the actual image resize and
    // lossless-WebP encoder, not an already-encoded cached variant.
    let image = RgbImage::from_fn(720, 1080, |x, y| {
        Rgb([
            (x % 256) as u8,
            (y % 256) as u8,
            ((x / 3 + y / 7) % 256) as u8,
        ])
    });
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut output, ImageFormat::Png)
        .expect("encode test cover PNG");
    output.into_inner()
}

async fn call(
    app: &Router,
    path: &str,
    cookies: &str,
    payload: Option<Value>,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .uri(path)
        .method(if payload.is_some() { "POST" } else { "GET" })
        .header(header::COOKIE, cookies);
    if payload.is_some() {
        request = request
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(payload.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .expect("Axum response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded response");
    (
        status,
        String::from_utf8(bytes.to_vec()).expect("UTF-8 response"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated PostgreSQL 18 /tor_ci and fake S3; no retained databases"]
async fn s023_real_worker_and_author_previews_share_bounded_pool() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never run on a retained database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let author = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("s023-real-load-{nonce}@tor-ci.example"),
            "S023 Load Author",
            &hash_password("S023ActualWorker2026").unwrap(),
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book = new_book(
        &format!("tor-ci:s023-worker-load:{nonce}"),
        "ru",
        "Нагрузка на фоновый импорт",
        &"a".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let store = MemoryStore::default();
    let queue_app = tor_web::chapter_import_queue_http::router(
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
        Arc::new(store.clone()),
    );
    let public_app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookies = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");

    // The actual worker will deserialize, verify and atomically write
    // 400 chapters from a real S023 queue record in the disposable database.
    let text = (1..=400)
        .map(|number| format!("Глава {number}: Нагрузка\n{}\n", "x".repeat(1300)))
        .collect::<String>();
    assert!(text.len() > 500_000 && text.len() < 700_000);
    let chapters = parse_text("load.txt", &text).unwrap();
    let approved: String = fingerprint(book_id, &chapters)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let queue_path = format!("/me/books/{book_id}/chapters/import-queue");
    let (queued_status, queued_body) = call(
        &queue_app,
        &queue_path,
        &cookies,
        Some(json!({
            "filename": "load.txt",
            "text": text,
            "csrf_token": csrf,
            "command_key": generate_session_token(),
            "fingerprint": approved,
        })),
    )
    .await;
    assert_eq!(queued_status, StatusCode::OK, "{queued_body}");
    let job_id = serde_json::from_str::<Value>(&queued_body).unwrap()["job_id"]
        .as_i64()
        .unwrap();

    // Register the real G-09 cover job against the exact same author/book.
    // Moderation remains separate: processing never makes a cover public.
    let cover_store = MemoryCoverStore::default();
    let cover_bytes = cover_png();
    let cover_key = format!("staging/covers/ci-mixed-{nonce}-source.png");
    cover_store.add(&cover_key, cover_bytes.clone()).await;
    let cover_jobs = PgCoverJobs::new(pool.clone());
    let cover_hash: [u8; 32] = Sha256::digest(&cover_bytes).into();
    let source_id = cover_jobs
        .register_verified_source(
            book_id,
            owner,
            &cover_key,
            "image/png",
            cover_bytes.len() as i64,
            &cover_hash,
        )
        .await
        .expect("register verified private cover source");
    let cover_job_id = cover_jobs
        .enqueue(source_id, book_id, owner, 0, RECIPE_VERSION)
        .await
        .expect("enqueue private G-09 worker job");

    // Real REC worker: schedule a generation before contention, then advance
    // one bounded page at the same start gate as the other workers.
    let recommendations = PgRecommendations::new(pool.clone());
    recommendations
        .request_refresh()
        .await
        .expect("schedule real recommendation generation");

    // A disposable wallet with a known value lets finance reads prove both
    // correctness and absence of unintended balance changes under contention.
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES($1,1000,'RUB')",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .expect("seed isolated creator wallet");

    const PREVIEWS: usize = 18;
    const FINANCE_READS: usize = 8;
    // Previews + importer + cover + REC + finance readers + health probe.
    let barrier = Arc::new(Barrier::new(PREVIEWS + FINANCE_READS + 4));
    let worker = ChapterImportWorker::new(&catalog, Arc::new(store.clone()));
    let worker_gate = Arc::clone(&barrier);
    let worker_task = tokio::spawn(async move {
        worker_gate.wait().await;
        let started = Instant::now();
        let result = worker.run_one().await.expect("actual S023 worker");
        (result, started.elapsed())
    });

    let cover_monitor = CoverRunnerMonitor::default();
    let cover_runner = CoverRunner::new(
        catalog.clone(),
        Arc::new(cover_store.clone()),
        cover_monitor.clone(),
    );
    let cover_gate = Arc::clone(&barrier);
    let cover_task = tokio::spawn(async move {
        cover_gate.wait().await;
        let started = Instant::now();
        let completed = cover_runner.run_one_bounded(Duration::from_secs(20)).await;
        (completed, started.elapsed())
    });

    let recommendation_gate = Arc::clone(&barrier);
    let recommendation_task = tokio::spawn(async move {
        recommendation_gate.wait().await;
        let started = Instant::now();
        let advanced = recommendations
            .advance_generation()
            .await
            .expect("real recommendation worker page");
        (advanced, started.elapsed())
    });

    let finance = PgCreatorFinance::new(pool.clone());
    let mut finance_tasks = tokio::task::JoinSet::new();
    for _ in 0..FINANCE_READS {
        let reader = finance.clone();
        let gate = Arc::clone(&barrier);
        finance_tasks.spawn(async move {
            gate.wait().await;
            let started = Instant::now();
            let totals = reader.totals(owner).await.expect("real finance totals");
            (totals, started.elapsed())
        });
    }

    let mut pending = tokio::task::JoinSet::new();
    let preview_path = format!("/me/books/{book_id}/chapters/import-preview");
    let preview_text = "Глава 900: Проверка\nТекст без записи";
    for _ in 0..PREVIEWS {
        let app = public_app.clone();
        let path = preview_path.clone();
        let cookie = cookies.clone();
        let csrf = csrf.clone();
        let gate = Arc::clone(&barrier);
        pending.spawn(async move {
            gate.wait().await;
            let started = Instant::now();
            let (status, body) = call(
                &app,
                &path,
                &cookie,
                Some(json!({
                    "filename": "preview.txt",
                    "text": preview_text,
                    "csrf_token": csrf,
                })),
            )
            .await;
            (status, body, started.elapsed())
        });
    }

    // Liveness probes run on the same Axum router and the same PgPool as the
    // real worker and authenticated previews, not a separately opened pool.
    let probe_app = public_app.clone();
    let probe_pool = pool.clone();
    let probe_gate = Arc::clone(&barrier);
    let probes = tokio::spawn(async move {
        probe_gate.wait().await;
        let mut health = Vec::new();
        let mut ready = Vec::new();
        let mut acquire = Vec::new();
        for index in 0..80 {
            let start = Instant::now();
            let (status, _) = call(&probe_app, "/healthz", "", None).await;
            assert_eq!(status, StatusCode::OK);
            health.push(start.elapsed());
            if index % 5 == 0 {
                let start = Instant::now();
                let (status, _) = call(&probe_app, "/readyz", "", None).await;
                assert_eq!(status, StatusCode::OK);
                ready.push(start.elapsed());
            }
            if index % 2 == 0 {
                // Measure *pool acquisition wait*, not SQL query execution.
                // All 3 shared connections are available to G-09, S023,
                // authenticated HTTP and this probe; no separate test pool.
                let start = Instant::now();
                let connection = tokio::time::timeout(Duration::from_secs(3), probe_pool.acquire())
                    .await
                    .expect("shared PostgreSQL pool acquire timeout")
                    .expect("shared PostgreSQL connection available");
                acquire.push(start.elapsed());
                drop(connection);
            }
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        (health, ready, acquire)
    });

    let mut successes = 0_usize;
    let mut rejected = 0_usize;
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(joined) = pending.join_next().await {
            let (status, body, _) = joined.expect("preview task joined");
            match status {
                StatusCode::OK => {
                    let preview: Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(preview["count"], 1);
                    successes += 1;
                }
                StatusCode::TOO_MANY_REQUESTS => rejected += 1,
                unexpected => panic!("unexpected authenticated preview {unexpected}: {body}"),
            }
        }
    })
    .await
    .expect("concurrent HTTP previews finished");
    let (worker_status, worker_elapsed) =
        tokio::time::timeout(Duration::from_secs(20), worker_task)
            .await
            .expect("worker did not hang")
            .expect("worker joined");
    let (cover_completed, cover_elapsed) =
        tokio::time::timeout(Duration::from_secs(25), cover_task)
            .await
            .expect("cover worker did not hang")
            .expect("cover runner joined");
    let (recommendation_advanced, recommendation_elapsed) =
        tokio::time::timeout(Duration::from_secs(25), recommendation_task)
            .await
            .expect("recommendation worker did not hang")
            .expect("recommendation worker joined");
    let mut finance_elapsed = Vec::new();
    while let Some(joined) =
        tokio::time::timeout(Duration::from_secs(25), finance_tasks.join_next())
            .await
            .expect("finance reader did not hang")
    {
        let (totals, elapsed) = joined.expect("finance reader joined");
        assert_eq!(totals.wallet_balance_minor, 1000);
        assert_eq!(totals.available_minor, 1000);
        assert_eq!(totals.reserved_minor, 0);
        finance_elapsed.push(elapsed);
    }
    assert_eq!(finance_elapsed.len(), FINANCE_READS);
    finance_elapsed.sort_unstable();
    let finance_p95 = finance_elapsed[finance_elapsed.len() * 95 / 100];
    let (mut health, mut ready, mut acquire) = probes.await.expect("liveness probe joined");
    health.sort_unstable();
    ready.sort_unstable();
    acquire.sort_unstable();
    let health_p95 = health[health.len() * 95 / 100];
    let ready_p95 = ready[ready.len() * 95 / 100];
    let acquire_p95 = acquire[acquire.len() * 95 / 100];
    let acquire_max = acquire[acquire.len() - 1];

    println!(
        "TOR_MIXED_REC_FINANCE_G09_S023_CONTENTION previews={} success={} backpressure_429={} finance_reads={} finance_p95_ms={} rec_worker_ms={} import_worker_ms={} cover_worker_ms={} health_p95_ms={} ready_p95_ms={} pg_acquire_p95_ms={} pg_acquire_max_ms={}",
        PREVIEWS, successes, rejected, FINANCE_READS, finance_p95.as_millis(),
        recommendation_elapsed.as_millis(), worker_elapsed.as_millis(),
        cover_elapsed.as_millis(), health_p95.as_millis(), ready_p95.as_millis(),
        acquire_p95.as_millis(), acquire_max.as_millis()
    );
    assert_eq!(successes + rejected, PREVIEWS);
    assert!(successes > 0, "at least one preview must be served");
    assert_eq!(worker_status, ChapterImportWorkerResult::Completed);
    assert!(
        recommendation_advanced,
        "recommendation generation must advance"
    );
    assert!(
        finance_p95 < Duration::from_secs(2),
        "finance balance reads saturated the shared PgPool"
    );
    assert!(
        health_p95 < Duration::from_millis(250),
        "worker starved Tokio"
    );
    assert!(
        ready_p95 < Duration::from_secs(2),
        "workers blocked shared PgPool"
    );
    assert!(
        acquire_p95 < Duration::from_secs(2),
        "three-connection PgPool saturated for too long"
    );
    assert!(cover_completed, "cover worker timed out");
    let cover_snapshot = cover_monitor.snapshot();
    assert_eq!(cover_snapshot.attached, 1, "cover candidate must be staged");
    assert_eq!(cover_snapshot.errors, 0);
    assert_eq!(cover_snapshot.timeouts, 0);
    assert_eq!(
        cover_jobs
            .job_status(cover_job_id)
            .await
            .unwrap()
            .as_deref(),
        Some("done")
    );
    let cover_variants = cover_jobs.existing_variants(cover_job_id).await.unwrap();
    assert_eq!(
        cover_variants.len(),
        2,
        "WebP main/card variants must exist"
    );
    for variant in &cover_variants {
        assert!(
            cover_store
                .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
                .await
                .unwrap(),
            "cover variant must match the registered SHA-256"
        );
    }
    assert!(
        tor_books::repositories::cover_slots::PgCoverSlots::new(pool.clone())
            .current_slot(book_id)
            .await
            .unwrap()
            .is_none(),
        "queued moderation must not expose a private cover"
    );
    // The legacy list_owned_chapters API deliberately caps results at 200.
    // Check every page using the canonical owner-scoped keyset cursor.
    let mut cursor = None;
    let mut saved_numbers = Vec::new();
    loop {
        let page = author
            .list_owned_chapters_page(owner, book_id, cursor)
            .await
            .unwrap();
        saved_numbers.extend(page.chapters.iter().map(|chapter| chapter.number));
        assert!(saved_numbers.len() <= 400, "unexpected duplicate pages");
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(saved_numbers, (1..=400).collect::<Vec<i32>>());
    assert!(store.0.lock().await.is_empty(), "private source cleaned up");
    let status_path = format!("/me/books/{book_id}/chapters/import-jobs/{job_id}");
    let (status, body) = call(&queue_app, &status_path, &cookies, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["status"],
        "succeeded"
    );
    pool.close().await;
}
