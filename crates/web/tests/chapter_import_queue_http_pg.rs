//! S023 HTTP -> private object store -> PostgreSQL queue -> worker regression.
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
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
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
use tor_web::chapter_import_worker::{ChapterImportWorker, ChapterImportWorkerResult};
use tor_imports::services::{ChapterImportObjectStore, ChapterImportStoreError as StoreError};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

#[derive(Clone, Default)]
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
        let mut objects = self.0.lock().await;
        if let Some(existing) = objects.get(key) {
            return if existing == bytes {
                Ok(())
            } else {
                Err(StoreError::Conflict)
            };
        }
        objects.insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    async fn get_verified(
        &self,
        key: &str,
        expected_size: i64,
        sha256: &[u8; 32],
        max_bytes: usize,
    ) -> Result<Vec<u8>, StoreError> {
        let objects = self.0.lock().await;
        let bytes = objects.get(key).ok_or(StoreError::Unavailable)?;
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if bytes.len() != expected_size as usize
            || bytes.len() > max_bytes
            || actual != *sha256
        {
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
    method: &str,
    cookies: &str,
    json_body: Option<Value>,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookies);
    if method == "POST" {
        request = request
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/json");
    }
    let body = json_body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let text = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, text)
}

fn fingerprint_hex(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn s023_queue_http_worker_status_and_cancel_are_owner_scoped() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "no production database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let authoring = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("S023QueueIntegration2026").unwrap();
    let owner_token = generate_session_token();
    let stranger_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("s023queue-{nonce}@tor-ci.example"),
            "Queue owner",
            &password,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let _stranger = accounts
        .register_with_session(
            &format!("s023stranger-{nonce}@tor-ci.example"),
            "Stranger",
            &password,
            &session_token_digest(&stranger_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book = new_book(
        &format!("tor-ci:s023-queue:{nonce}"),
        "ru",
        "Фоновый импорт",
        &"q".repeat(43),
    )
    .unwrap();
    let book_id = authoring.create_owned_book(owner, &book).await.unwrap();
    let store = MemoryStore::default();
    let app = tor_web::chapter_import_queue_http::router(
        accounts,
        false,
        ORIGIN.to_owned(),
        Arc::new(store.clone()),
    );
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let stranger_cookie = format!("tor-dev-session={stranger_token}; tor-dev-csrf={csrf}");

    let cancelled_text = "Глава 1: Отмена\nТекст";
    let cancelled_chapters = parse_text("cancel.txt", cancelled_text).unwrap();
    let cancelled_fp = fingerprint(book_id, &cancelled_chapters);
    let cancelled_key = generate_session_token();
    let cancelled_payload = json!({
        "filename":"cancel.txt",
        "text":cancelled_text,
        "csrf_token":csrf,
        "command_key":cancelled_key,
        "fingerprint":fingerprint_hex(&cancelled_fp),
    });
    let queue_path = format!("/me/books/{book_id}/chapters/import-queue");
    assert_eq!(
        call(&app, &queue_path, "POST", &owner_cookie, Some(json!({
            "filename":"cancel.txt","text":cancelled_text,"csrf_token":csrf,
            "command_key":cancelled_key,"fingerprint":fingerprint_hex(&cancelled_fp)
        })))
        .await
        .0,
        StatusCode::OK
    );
    let queued = call(
        &app,
        &queue_path,
        "POST",
        &owner_cookie,
        Some(cancelled_payload.clone()),
    )
    .await;
    assert_eq!(queued.0, StatusCode::OK, "{}", queued.1);
    let queued_json: Value = serde_json::from_str(&queued.1).unwrap();
    let cancelled_job = queued_json["job_id"].as_i64().unwrap();
    assert_eq!(queued_json["status"], "queued");
    let status_path =
        format!("/me/books/{book_id}/chapters/import-jobs/{cancelled_job}");
    assert_eq!(
        call(&app, &status_path, "GET", &stranger_cookie, None)
            .await
            .0,
        StatusCode::NOT_FOUND,
        "another account cannot observe a private job"
    );
    let latest = call(
        &app,
        &format!("/me/books/{book_id}/chapters/import-jobs/latest"),
        "GET",
        &owner_cookie,
        None,
    )
    .await;
    assert_eq!(latest.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&latest.1).unwrap()["job_id"],
        cancelled_job
    );
    let cancel_path =
        format!("/me/books/{book_id}/chapters/import-jobs/{cancelled_job}/cancel");
    let cancelled = call(
        &app,
        &cancel_path,
        "POST",
        &owner_cookie,
        Some(json!({"csrf_token":csrf})),
    )
    .await;
    assert_eq!(cancelled.0, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&cancelled.1).unwrap()["cancelled"],
        true
    );
    assert_eq!(
        serde_json::from_str::<Value>(
            &call(&app, &status_path, "GET", &owner_cookie, None)
                .await
                .1
        )
        .unwrap()["status"],
        "cancelled"
    );

    // A second, reviewed batch is processed by the real worker against the
    // fake S3 port. The job and every chapter become successful together.
    let text = "Глава 1: Готово\nСодержимое";
    let chapters = parse_text("approved.md", text).unwrap();
    let approved = fingerprint(book_id, &chapters);
    let command_key = generate_session_token();
    let payload = json!({
        "filename":"approved.md",
        "text":text,
        "csrf_token":csrf,
        "command_key":command_key,
        "fingerprint":fingerprint_hex(&approved),
    });
    let response = call(&app, &queue_path, "POST", &owner_cookie, Some(payload)).await;
    assert_eq!(response.0, StatusCode::OK, "{}", response.1);
    let job_id = serde_json::from_str::<Value>(&response.1).unwrap()["job_id"]
        .as_i64()
        .unwrap();
    let worker = ChapterImportWorker::new(&catalog, Arc::new(store.clone()));
    assert_eq!(
        worker.run_one().await.unwrap(),
        ChapterImportWorkerResult::Completed
    );
    let status_path = format!("/me/books/{book_id}/chapters/import-jobs/{job_id}");
    let final_status = call(&app, &status_path, "GET", &owner_cookie, None).await;
    assert_eq!(final_status.0, StatusCode::OK);
    let status_json: Value = serde_json::from_str(&final_status.1).unwrap();
    assert_eq!(status_json["status"], "succeeded");
    assert_eq!(status_json["saved_count"], 1);
    assert_eq!(status_json["book_url"], format!("/me/books/{book_id}"));
    assert_eq!(
        authoring.list_owned_chapters(owner, book_id).await.unwrap().len(),
        1
    );
    assert!(store.0.lock().await.is_empty(), "completed and cancelled source objects are cleaned up");
    pool.close().await;
}
