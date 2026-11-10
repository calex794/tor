//! P1: actual S023 background importer and authenticated HTTP previews compete
//! for the SAME three-connection PostgreSQL pool, with a fake S3 object port.
//! No external storage, production data, deployments or schema mutations.
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
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
use tor_imports::services::{ChapterImportObjectStore, ChapterImportStoreError as StoreError};
use tor_reading::repositories::PgReader;
use tor_web::chapter_import_worker::{ChapterImportWorker, ChapterImportWorkerResult};
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
        .oneshot(request.body(payload.map_or_else(Body::empty, |body| Body::from(body.to_string()))).unwrap())
        .await
        .expect("Axum response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("bounded response");
    (status, String::from_utf8(bytes.to_vec()).expect("UTF-8 response"))
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

    const PREVIEWS: usize = 18;
    let barrier = Arc::new(Barrier::new(PREVIEWS + 2));
    let worker = ChapterImportWorker::new(&catalog, Arc::new(store.clone()));
    let worker_gate = Arc::clone(&barrier);
    let worker_task = tokio::spawn(async move {
        worker_gate.wait().await;
        let started = Instant::now();
        let result = worker.run_one().await.expect("actual S023 worker");
        (result, started.elapsed())
    });

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
    let probe_gate = Arc::clone(&barrier);
    let probes = tokio::spawn(async move {
        probe_gate.wait().await;
        let mut health = Vec::new();
        let mut ready = Vec::new();
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
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        (health, ready)
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
    let (mut health, mut ready) = probes.await.expect("liveness probe joined");
    health.sort_unstable();
    ready.sort_unstable();
    let health_p95 = health[health.len() * 95 / 100];
    let ready_p95 = ready[ready.len() * 95 / 100];

    println!(
        "TOR_REAL_WORKER_CONTENTION previews={} success={} backpressure_429={} worker_ms={} health_p95_ms={} ready_p95_ms={}",
        PREVIEWS, successes, rejected, worker_elapsed.as_millis(),
        health_p95.as_millis(), ready_p95.as_millis()
    );
    assert_eq!(successes + rejected, PREVIEWS);
    assert!(successes > 0, "at least one preview must be served");
    assert_eq!(worker_status, ChapterImportWorkerResult::Completed);
    assert!(health_p95 < Duration::from_millis(250), "worker starved Tokio");
    assert!(ready_p95 < Duration::from_secs(2), "worker blocked shared PgPool");
    assert_eq!(author.list_owned_chapters(owner, book_id).await.unwrap().len(), 400);
    assert!(store.0.lock().await.is_empty(), "private source cleaned up");
    let status_path = format!("/me/books/{book_id}/chapters/import-jobs/{job_id}");
    let (status, body) = call(&queue_app, &status_path, &cookies, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["status"], "succeeded");
    pool.close().await;
}
