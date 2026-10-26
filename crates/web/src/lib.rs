//! Routing shell only: book business logic remains in tor-books.
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Html,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tor_books::{
    repositories::PgCatalog,
    services::{
        CatalogError, GetPublicBook, ListPublicBooks, PublicBookRepository, DEFAULT_PAGE_SIZE,
    },
};
use tor_reading::{
    repositories::PgReader,
    services::{PublicReadingRepository, ReadingError},
};
mod auth;
mod auth_cookies;
mod author_pages;
pub mod cover_worker;
mod pages;
mod password_jobs;
mod ril;
mod safe_markdown;

#[derive(Clone)]
struct AppState {
    books: Arc<dyn PublicBookRepository>,
    postgres: Option<PgCatalog>,
    reader: Option<PgReader>,
}

async fn health() -> &'static str {
    "ok"
}
/// Readiness uses the exact PG schema and database permissions, not only the process.
async fn ready(State(state): State<AppState>) -> (StatusCode, &'static str) {
    if let Some(catalog) = state.postgres {
        if catalog.probe().await.is_err() {
            return (StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    }
    if let Some(reader) = state.reader {
        if reader.probe().await.is_err() {
            return (StatusCode::SERVICE_UNAVAILABLE, "unavailable");
        }
    }
    (StatusCode::OK, "ok") // demo mode has neither PG adapter
}

async fn public_cards(
    repo: &dyn PublicBookRepository,
    limit: usize,
) -> Result<Vec<pages::PublicBookCard>, StatusCode> {
    let result = ListPublicBooks::new(repo)
        .execute(limit)
        .await
        .map_err(|error| match error {
            CatalogError::InvalidLimit => StatusCode::BAD_REQUEST,
            CatalogError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    Ok(result
        .into_iter()
        .map(|book| pages::PublicBookCard {
            id: book.id(),
            title: book.title().to_owned(),
            language: book.key().target_language().to_owned(),
        })
        .collect())
}

async fn homepage(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    Ok(pages::home(&public_cards(state.books.as_ref(), 6).await?))
}
async fn catalog(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    Ok(pages::catalog(
        &public_cards(state.books.as_ref(), DEFAULT_PAGE_SIZE).await?,
    ))
}
#[derive(Default, Deserialize)]
struct TocQuery {
    limit: Option<u16>,
    after: Option<String>,
}

#[derive(Serialize)]
struct TocItem {
    chapter_id: String,
    chapter_no: i32,
    title: String,
    access_level: &'static str,
}

#[derive(Serialize)]
struct TocResponse {
    items: Vec<TocItem>,
    next_cursor: Option<String>,
}

fn reading_status(error: ReadingError) -> StatusCode {
    match error {
        ReadingError::InvalidPage => StatusCode::BAD_REQUEST,
        ReadingError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// C-03: bounded chapter metadata, no unpublished chapter or paid body content.
async fn book_toc(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<TocQuery>,
) -> Result<Json<TocResponse>, StatusCode> {
    let reader = state.reader.ok_or(StatusCode::NOT_FOUND)?;
    let book = reader
        .public_book_page(id, query.limit.unwrap_or(100), query.after.as_deref())
        .await
        .map_err(reading_status)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let items = book
        .chapters
        .into_iter()
        .map(|chapter| TocItem {
            chapter_id: chapter.id.to_string(),
            chapter_no: chapter.number,
            title: chapter.title,
            access_level: if chapter.is_paid { "paid" } else { "free" },
        })
        .collect();
    Ok(Json(TocResponse {
        items,
        next_cursor: book.next_cursor,
    }))
}

async fn book_detail(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<TocQuery>,
) -> Result<Html<String>, StatusCode> {
    if let Some(reader) = state.reader {
        let limit = query.limit.unwrap_or(100);
        let book = reader
            .public_book_page(id, limit, query.after.as_deref())
            .await
            .map_err(reading_status)?
            .ok_or(StatusCode::NOT_FOUND)?;
        return Ok(pages::book_detail_with_chapters(&book, limit));
    }
    let repo = state.books.as_ref();
    let book = GetPublicBook::new(repo)
        .execute(id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(pages::book_detail(&pages::PublicBookCard {
        id: book.id(),
        title: book.title().into(),
        language: book.key().target_language().into(),
    }))
}
async fn reader_page(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    let reader = state.reader.ok_or(StatusCode::NOT_FOUND)?;
    let chapter = reader
        .public_chapter(id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok((
        [
            ("cache-control", "private, no-store"),
            ("content-security-policy", "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"),
            ("x-content-type-options", "nosniff"),
        ],
        pages::reader(&chapter),
    ))
}
async fn login() -> impl axum::response::IntoResponse {
    pages::inactive_account_page("Вход")
}
async fn register() -> impl axum::response::IntoResponse {
    pages::inactive_account_page("Регистрация")
}

/// Optional, non-blocking persistence of anonymous technical RIL aggregates.
/// Uses the already bounded PostgreSQL pool and never changes readiness/ACL.
/// Internal aggregate diagnostics only; no user keys or route sequences.
/// This is not an HTTP endpoint.
pub fn ril_path_report_for_diagnostics() -> Option<tor_traffic_behavior::PathReport> {
    ril::path_report_for_diagnostics()
}

pub fn start_ril_pg_background(catalog: &PgCatalog) {
    ril::start_pg_background(tor_traffic_behavior::repositories::PgRilRollups::new(
        catalog.pool(),
    ));
}

pub fn router(repo: Arc<dyn PublicBookRepository>) -> Router {
    ril::instrument(make_router(repo, None, None, false))
}
pub fn router_with_postgres(catalog: PgCatalog) -> Router {
    ril::instrument(make_router(
        Arc::new(catalog.clone()),
        Some(catalog),
        None,
        false,
    ))
}
pub fn router_with_postgres_reading(catalog: PgCatalog, reader: PgReader) -> Router {
    ril::instrument(make_router(
        Arc::new(catalog.clone()),
        Some(catalog),
        Some(reader),
        false,
    ))
}

/// Account forms are gated until their database, CSRF and rate-limit tests pass.
pub fn router_with_postgres_reading_auth(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
) -> Router {
    let auth_reader = reader.clone();
    ril::instrument(
        make_router(Arc::new(catalog.clone()), Some(catalog), Some(reader), true)
            .merge(auth::router(accounts, auth_reader, secure, public_origin)),
    )
}

fn make_router(
    repo: Arc<dyn PublicBookRepository>,
    postgres: Option<PgCatalog>,
    reader: Option<PgReader>,
    active_auth: bool,
) -> Router {
    let api = tor_books::api::router(repo.clone());
    let mut app = Router::new()
        .route("/", get(homepage))
        .route("/catalog", get(catalog))
        .route("/books/{id}", get(book_detail))
        .route("/api/v1/books/{id}/toc", get(book_toc))
        .route("/reader/{id}", get(reader_page))
        .route("/assets/site.css", get(pages::stylesheet))
        .route("/healthz", get(health))
        .route("/readyz", get(ready));
    if !active_auth {
        app = app
            .route("/login", get(login))
            .route("/register", get(register));
    }
    app.with_state(AppState {
        books: repo,
        postgres,
        reader,
    })
    .merge(api)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tor_books::repositories::InMemoryCatalog;
    use tower::ServiceExt;
    async fn get(path: &str) -> (StatusCode, String) {
        let app = router(Arc::new(InMemoryCatalog::demo()));
        let response = app
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }
    #[tokio::test]
    async fn demo_serves_real_html_routes() {
        for path in [
            "/",
            "/catalog",
            "/books/1",
            "/login",
            "/register",
            "/healthz",
            "/readyz",
            "/assets/site.css",
        ] {
            assert_eq!(get(path).await.0, StatusCode::OK, "{path}");
        }
    }
    #[tokio::test]
    async fn hidden_book_never_reaches_public_pages() {
        for path in ["/", "/catalog", "/api/v1/books"] {
            let (_, body) = get(path).await;
            assert!(body.contains("Первое знакомство"));
            assert!(!body.contains("Закрытый черновик"));
        }
        assert_eq!(get("/books/3").await.0, StatusCode::NOT_FOUND);
    }
    #[tokio::test]
    async fn demo_must_not_advertise_unimplemented_reader() {
        assert_eq!(get("/reader/1").await.0, StatusCode::NOT_FOUND);
    }
    #[tokio::test]
    async fn no_user_or_finance_write_routes() {
        assert_eq!(
            get("/api/v1/books?limit=10000").await.0,
            StatusCode::BAD_REQUEST
        );
        let app = router(Arc::new(InMemoryCatalog::demo()));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
