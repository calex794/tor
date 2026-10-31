//! S023 owner-only queue endpoints. No S3 key or object URL leaves this module.
use axum::{
    extract::{DefaultBodyLimit, Json, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::session_token_digest,
};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{valid_command_key, AuthorError},
        chapter_file_import::{fingerprint, parse_text, ImportedChapter},
    },
};
use std::sync::Arc;
use tor_imports::{
    domain::{ChapterImportStatus, ImportJobsError},
    repositories::postgres::PgImportJobs,
    services::{ChapterImportObjectStore, ChapterImportStoreError},
};

use crate::auth_cookies::{private_response, CookiePolicy};

#[derive(Clone)]
struct QueueState {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    jobs: PgImportJobs,
    cookies: CookiePolicy,
    storage: Arc<dyn ChapterImportObjectStore>,
}

#[derive(Deserialize)]
struct QueueInput {
    filename: String,
    text: String,
    csrf_token: String,
    command_key: String,
    fingerprint: String,
}

#[derive(Deserialize)]
struct CancelInput {
    csrf_token: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: &'static str,
}

#[derive(Serialize)]
struct QueueResponse {
    job_id: i64,
    status: String,
    status_url: String,
}

#[derive(Serialize)]
struct StatusResponse {
    job_id: i64,
    status: String,
    attempts: i32,
    saved_count: i32,
    error_code: Option<String>,
    book_url: Option<String>,
}

#[derive(Serialize)]
struct CancelResponse {
    cancelled: bool,
}

pub fn router(
    accounts: PgAccounts,
    secure: bool,
    origin: String,
    storage: Arc<dyn ChapterImportObjectStore>,
) -> Router {
    let pool = accounts.pool();
    let authoring = PgAuthoring::new(pool.clone());
    let jobs = PgImportJobs::new(pool.clone());
    let state = QueueState {
        accounts,
        authoring,
        jobs,
        cookies: CookiePolicy {
            secure,
            origin,
        },
        storage,
    };
    Router::new()
        .route(
            "/me/books/{book_id}/chapters/import-queue",
            post(enqueue),
        )
        .route(
            "/me/books/{book_id}/chapters/import-jobs/latest",
            get(latest),
        )
        .route(
            "/me/books/{book_id}/chapters/import-jobs/{job_id}",
            get(status),
        )
        .route(
            "/me/books/{book_id}/chapters/import-jobs/{job_id}/cancel",
            post(cancel),
        )
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}

async fn actor(state: &QueueState, headers: &HeaderMap) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

fn fail(status: StatusCode, message: &'static str) -> axum::response::Response {
    private_response((status, Json(ErrorResponse { error: message })), &[])
}

fn parse_hex_hash(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut result = [0_u8; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(value.get(start..start + 2)?, 16).ok()?;
    }
    Some(result)
}

fn status_response(job_id: i64, status: ChapterImportStatus, book_id: i64) -> StatusResponse {
    let succeeded = status.status == "succeeded";
    StatusResponse {
        job_id,
        status: status.status,
        attempts: status.attempts,
        saved_count: status.saved_count,
        error_code: status.error_code,
        book_url: succeeded.then(|| format!("/me/books/{book_id}")),
    }
}

async fn enqueue(
    State(state): State<QueueState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Json(input): Json<QueueInput>,
) -> axum::response::Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, &input.csrf_token)
    {
        return fail(StatusCode::FORBIDDEN, "Проверка запроса не пройдена.");
    }
    let actor_id = match actor(&state, &headers).await {
        Ok(Some(actor_id)) => actor_id,
        Ok(None) => return fail(StatusCode::UNAUTHORIZED, "Требуется войти в аккаунт."),
        Err(status) => return fail(status, "Сервис временно недоступен."),
    };
    if book_id <= 0 || !valid_command_key(&input.command_key) {
        return fail(StatusCode::BAD_REQUEST, "Некорректный запрос импорта.");
    }
    match state.authoring.owned_book(actor_id, book_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::NOT_FOUND, "Книга не найдена."),
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "База данных недоступна."),
    }
    match state.authoring.owned_book_is_public(actor_id, book_id).await {
        Ok(false) => {}
        Ok(true) => return fail(StatusCode::CONFLICT, "Импорт доступен только для черновика."),
        Err(AuthorError::Missing) => return fail(StatusCode::NOT_FOUND, "Книга не найдена."),
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "База данных недоступна."),
    }

    // The browser has already shown this exact canonical UTF-8 text to the
    // author. Archives are converted to the same editable text before enqueue.
    let chapters: Vec<ImportedChapter> = match parse_text(&input.filename, &input.text) {
        Ok(chapters) => chapters,
        Err(_) => return fail(StatusCode::UNPROCESSABLE_ENTITY, "Проверьте формат и структуру глав."),
    };
    let approved = fingerprint(book_id, &chapters);
    let Some(expected) = parse_hex_hash(&input.fingerprint) else {
        return fail(StatusCode::BAD_REQUEST, "Некорректная подпись предпросмотра.");
    };
    if expected != approved {
        return fail(StatusCode::CONFLICT, "Текст изменился после предпросмотра. Проверьте его заново.");
    }
    let bytes = input.text.as_bytes();
    if bytes.is_empty() || bytes.len() > 700_000 {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "Текст превышает допустимый размер.");
    }
    let source_hash: [u8; 32] = Sha256::digest(bytes).into();

    // Replay lookup comes before S3 so a completed retry does not recreate an
    // already-cleaned source object. A reused key with changed content is 409.
    let existing = match state
        .jobs
        .existing_for_command(
            actor_id,
            book_id,
            &input.command_key,
            &input.filename,
            &source_hash,
            bytes.len() as i64,
            &approved,
            chapters.len() as i32,
        )
        .await
    {
        Ok(existing) => existing,
        Err(ImportJobsError::Conflict) => return fail(StatusCode::CONFLICT, "Ключ уже использован с другими данными."),
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE, "Не удалось проверить повтор запроса."),
    };
    if let Some(job_id) = existing {
        return match state
            .jobs
            .status_for_owner(actor_id, book_id, job_id)
            .await
        {
            Ok(Some(job)) => private_response(
                Json(QueueResponse {
                    job_id,
                    status: job.status,
                    status_url: format!("/me/books/{book_id}/chapters/import-jobs/{job_id}"),
                }),
                &[],
            ),
            _ => fail(StatusCode::SERVICE_UNAVAILABLE, "Не удалось прочитать состояние задания."),
        };
    }

    let key = format!(
        "staging/chapters/{actor_id}/{book_id}/{}",
        input.command_key
    );
    if let Err(error) = state.storage.put_verified(&key, bytes, &source_hash).await {
        let (code, message) = match error {
            ChapterImportStoreError::Invalid => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Источник импорта не прошёл проверку.",
            ),
            ChapterImportStoreError::Conflict => (
                StatusCode::CONFLICT,
                "Источник с таким ключом уже содержит другие данные.",
            ),
            ChapterImportStoreError::Corrupt | ChapterImportStoreError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Закрытое хранилище временно недоступно.",
            ),
        };
        return fail(code, message);
    }

    let job_id = match state
        .jobs
        .enqueue_verified(
            actor_id,
            book_id,
            &input.command_key,
            &key,
            &input.filename,
            &source_hash,
            bytes.len() as i64,
            &approved,
            chapters.len() as i32,
        )
        .await
    {
        Ok(job_id) => job_id,
        // Do not delete this deterministic S3 key on an uncertain database
        // outcome: another identical concurrent request may already have
        // committed the job and its worker may be reading the same object.
        // Unreferenced staging objects are removed by private bucket lifecycle.
        Err(ImportJobsError::Conflict) => {
            return fail(
                StatusCode::CONFLICT,
                "Для книги уже выполняется импорт или изменились данные.",
            );
        }
        Err(ImportJobsError::Invalid) => {
            return fail(StatusCode::UNPROCESSABLE_ENTITY, "Недопустимые данные импорта.");
        }
        Err(_) => {
            return fail(StatusCode::SERVICE_UNAVAILABLE, "Не удалось поставить импорт в очередь.");
        }
    };
    private_response(
        Json(QueueResponse {
            job_id,
            status: "queued".to_owned(),
            status_url: format!("/me/books/{book_id}/chapters/import-jobs/{job_id}"),
        }),
        &[],
    )
}

async fn latest(
    State(state): State<QueueState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> axum::response::Response {
    let actor_id = match actor(&state, &headers).await {
        Ok(Some(actor_id)) => actor_id,
        Ok(None) => return fail(StatusCode::UNAUTHORIZED, "Требуется войти в аккаунт."),
        Err(status) => return fail(status, "Сервис временно недоступен."),
    };
    match state
        .jobs
        .latest_active_for_owner(actor_id, book_id)
        .await
    {
        Ok(Some((job_id, job))) => {
            private_response(Json(status_response(job_id, job, book_id)), &[])
        }
        Ok(None) => fail(StatusCode::NOT_FOUND, "Активное задание не найдено."),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE, "База данных недоступна."),
    }
}

async fn status(
    State(state): State<QueueState>,
    Path((book_id, job_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> axum::response::Response {
    let actor_id = match actor(&state, &headers).await {
        Ok(Some(actor_id)) => actor_id,
        Ok(None) => return fail(StatusCode::UNAUTHORIZED, "Требуется войти в аккаунт."),
        Err(status) => return fail(status, "Сервис временно недоступен."),
    };
    match state
        .jobs
        .status_for_owner(actor_id, book_id, job_id)
        .await
    {
        Ok(Some(job)) => private_response(Json(status_response(job_id, job, book_id)), &[]),
        Ok(None) => fail(StatusCode::NOT_FOUND, "Задание не найдено."),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE, "База данных недоступна."),
    }
}

async fn cancel(
    State(state): State<QueueState>,
    Path((book_id, job_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Json(input): Json<CancelInput>,
) -> axum::response::Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(&headers, &input.csrf_token)
    {
        return fail(StatusCode::FORBIDDEN, "Проверка запроса не пройдена.");
    }
    let actor_id = match actor(&state, &headers).await {
        Ok(Some(actor_id)) => actor_id,
        Ok(None) => return fail(StatusCode::UNAUTHORIZED, "Требуется войти в аккаунт."),
        Err(status) => return fail(status, "Сервис временно недоступен."),
    };
    match state
        .jobs
        .cancel_source_for_owner(actor_id, book_id, job_id)
        .await
    {
        Ok(Some(key)) => {
            let _ = state.storage.delete(&key).await;
            private_response(Json(CancelResponse { cancelled: true }), &[])
        }
        Ok(None) => private_response(Json(CancelResponse { cancelled: false }), &[]),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE, "Не удалось отменить задание."),
    }
}
