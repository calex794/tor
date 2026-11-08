//! Routing shell only: book business logic remains in tor-books.
use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tor_books::{
    repositories::PgCatalog,
    services::{CatalogError, GetPublicBook, ListPublicBooks, PublicBookRepository},
};
use tor_reading::{repositories::PgReader, services::ReadingError};
pub mod achievement_consumer;
mod auth;
pub use auth::security_retention::start as start_account_security_retention;
mod auth_cookies;
mod author_chapter_http;
mod author_chapter_images;
mod author_chapter_pages;
mod author_cover_http;
mod author_pages;
mod book_error_pages;
mod book_follow_page;
mod book_rating_context;
mod book_related_public;
mod catalog_http;
mod chapter_import_assets;
pub mod chapter_import_queue_http;
pub mod chapter_import_worker;
pub mod cover_cdn_cleanup;
pub mod cover_cdn_config;
pub mod cover_cdn_lifecycle;
pub mod cover_cdn_purge;
pub mod cover_delivery;
mod cover_http;
pub mod cover_private_cleanup;
pub mod cover_private_source_cleanup;
mod homepage_updates;
mod layer_reader_assets;
mod layer_reader_pages;
mod recommendation_http;
pub mod recommendation_worker;
mod referral_book_http;
pub use cover_http::{
    router_with_postgres_reading_auth_cover_uploads, router_with_postgres_reading_auth_covers,
    router_with_postgres_reading_covers,
};
mod admin_reader_router;
pub mod cover_worker;
mod observability;
mod operator_overview;
mod operator_traces;
mod pages;
#[cfg(feature = "s3-paid")]
pub mod paid_author_assets;
#[cfg(feature = "s3-paid")]
pub mod paid_author_publish;
mod paid_grants_http;
mod paid_reader_router;
mod public_reader_http;
pub mod yandex_cdn_purge;
#[cfg(feature = "s3-paid")]
pub use paid_author_assets::PgPaidAuthorAssets;
#[cfg(feature = "s3-paid")]
pub use paid_author_publish::PgPaidAuthorPublisher;
#[cfg(feature = "s3-paid")]
pub use paid_reader_router::router_with_postgres_reading_auth_paid_grants_browser_author;
pub use paid_reader_router::{
    router_with_postgres_reading_auth_paid_grants,
    router_with_postgres_reading_auth_paid_grants_browser,
};
mod password_jobs;
pub mod ranking_consumer;
pub mod referral_consumer;
mod ril;
mod worker_health;
#[derive(Clone)]
struct AppState {
    books: Arc<dyn PublicBookRepository>,
    postgres: Option<PgCatalog>,
    reader: Option<PgReader>,
    search: Option<tor_search::PgSearch>,
    paid_batch_browser_enabled: bool,
    paid_checkout_browser: bool,
    private_admin_reader_enabled: bool,
    private_admin_sealed_reader_enabled: bool,
    follow_control: Option<book_follow_page::FollowBookControl>,
    covers: Option<cover_delivery::CoverDelivery>,
}
/// Wiring mode is explicit; availability of a private admin shell does not
/// enable wallet checkout, encrypted S3 grants or Layer reading.
#[derive(Clone, Copy)]
struct ReaderModes {
    paid_browser: bool,
    admin_reader: bool,
}

async fn text_issue_script() -> Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/javascript; charset=utf-8",
        )],
        include_str!("../assets/newboo-text-issues.js"),
    )
        .into_response()
}
async fn health() -> &'static str {
    "ok"
}
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
            author_name: None,
            genres: Vec::new(),
        })
        .collect())
}
async fn homepage(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    homepage_updates::homepage(state).await
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

async fn book_toc(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<TocQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(reader) = state.reader.as_ref() else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let digest = public_reader_http::reader_session_digest(&state, &headers);
    let book = match reader
        .public_book_page_for_session(
            id,
            query.limit.unwrap_or(100),
            query.after.as_deref(),
            digest.as_ref(),
        )
        .await
    {
        Ok(Some(book)) => book,
        Ok(None) => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
        Err(error) => return auth_cookies::private_response(reading_status(error), &[]),
    };
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
    Json(TocResponse {
        items,
        next_cursor: book.next_cursor,
    })
    .into_response()
}

// H-11: never disclose whether a missing book was hidden or never existed.
async fn book_error(state: &AppState, status: StatusCode) -> Response {
    book_error_pages::public_book_error(state, status).await
}

async fn book_detail(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<TocQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Require the canonical numeric spelling before consulting public book data.
    if !tor_search::domain::seo_urls::BookUrl::from_id(id)
        .is_some_and(|book_url| book_url.matches_path(uri.path()))
    {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    }
    if let Some(reader) = state.reader.as_ref() {
        let limit = query.limit.unwrap_or(100);
        let digest = public_reader_http::reader_session_digest(&state, &headers);
        let book = match reader
            .public_book_page_for_session(id, limit, query.after.as_deref(), digest.as_ref())
            .await
        {
            Ok(Some(book)) => book,
            Ok(None) => return book_error(&state, StatusCode::NOT_FOUND).await,
            Err(error) => return book_error(&state, reading_status(error)).await,
        };
        let has_cover = if let Some(delivery) = state.covers.as_ref() {
            match delivery.available_card_ids(&[book.id]).await {
                Ok(ids) => ids.contains(&book.id),
                Err(_) => return book_error(&state, StatusCode::SERVICE_UNAVAILABLE).await,
            }
        } else {
            false
        };
        if let Some(catalog) = state.postgres.as_ref() {
            let id = match i64::try_from(book.id) {
                Ok(id) => id,
                Err(_) => return book_error(&state, StatusCode::NOT_FOUND).await,
            };
            let (stars, count) =
                match book_rating_context::for_book(catalog, id, digest.as_ref()).await {
                    Ok(Some(context)) => context,
                    Ok(None) => return book_error(&state, StatusCode::NOT_FOUND).await,
                    Err(status) => return book_error(&state, status).await,
                };
            let html = if state.paid_batch_browser_enabled {
                pages::book_detail_with_chapters_cover_stars_batch(
                    &book, limit, has_cover, &stars, true,
                )
            } else {
                pages::book_detail_with_chapters_cover_stars(&book, limit, has_cover, &stars)
            };
            let html = match book_related_public::attach(catalog, book.id, count, html).await {
                Ok(html) => html,
                Err(status) => return book_error(&state, status).await,
            };
            return cover_http::book_html(&state, &headers, book.id, html).await;
        }
        let html = pages::book_detail_with_chapters_cover(&book, limit, has_cover);
        let html = Html(html.0.replace("<!--BOOK_LIKES-->", ""));
        return cover_http::book_html(&state, &headers, book.id, html).await;
    }
    let repo = state.books.as_ref();
    let book = match GetPublicBook::new(repo).execute(id).await {
        Ok(Some(book)) => book,
        Ok(None) => return book_error(&state, StatusCode::NOT_FOUND).await,
        Err(_) => return book_error(&state, StatusCode::SERVICE_UNAVAILABLE).await,
    };
    let html = pages::book_detail(&pages::PublicBookCard {
        id: book.id(),
        title: book.title().into(),
        language: book.key().target_language().into(),
        author_name: None,
        genres: Vec::new(),
    });
    Html(
        html.0
            .replace("<!--BOOK_FOLLOW_ACTION-->", "")
            .replace("<!--BOOK_LIKES-->", ""),
    )
    .into_response()
}
async fn reader_page(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<public_reader_http::PublicIssueQuery>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
) -> Response {
    public_reader_http::reader_page(State(state), Path(id), Query(query), headers, method).await
}

async fn denied_reader_api(Path(_id): Path<u64>) -> Response {
    auth_cookies::private_response(StatusCode::NOT_FOUND, &[])
}
async fn login() -> impl axum::response::IntoResponse {
    pages::inactive_account_page("Вход")
}
async fn register() -> impl axum::response::IntoResponse {
    pages::inactive_account_page("Регистрация")
}

pub fn ril_path_report_for_diagnostics() -> Option<tor_traffic_behavior::PathReport> {
    ril::path_report_for_diagnostics()
}

pub async fn ril_cost_map_for_diagnostics(
    catalog: &PgCatalog,
    days: u16,
    limit: usize,
) -> Option<Vec<tor_route_intelligence::RouteCostRanking>> {
    use tor_route_intelligence::{rank_route_costs, RouteCostBucket};
    use tor_traffic_behavior::repositories::PgRilRollups;

    let rows = PgRilRollups::new(catalog.pool())
        .read_cost_buckets(days)
        .await
        .ok()?;
    let buckets: Vec<RouteCostBucket> = rows
        .into_iter()
        .map(|row| RouteCostBucket {
            route_code: row.route,
            status_group: row.status,
            latency_group: row.latency,
            request_count: row.count,
        })
        .collect();
    // Read SQLx evidence independently; if missing, keep the latency map
    // but leave query fields explicitly unmeasured.
    let mut ranked = rank_route_costs(&buckets, limit).ok()?;
    if let Ok(measured) = PgRilRollups::new(catalog.pool()).read_sql_costs(days).await {
        let evidence: Vec<tor_route_intelligence::RouteSqlEvidence> = measured
            .into_iter()
            .map(|row| tor_route_intelligence::RouteSqlEvidence {
                route_code: row.route,
                observed_requests: row.observed_requests,
                observed_queries: row.observed_queries,
                sqlx_elapsed_us: row.sqlx_elapsed_us,
            })
            .collect();
        if tor_route_intelligence::attach_sql_evidence(&mut ranked, &evidence).is_err() {
            return None; // Inconsistent PG evidence: no misleading report.
        }
    }
    Some(ranked)
}

/// Bounded process-local diagnostic traces, without user IDs or raw SQL.
/// This is an internal Rust API, not an unauthenticated HTTP endpoint.
pub fn trace_snapshots_for_diagnostics() -> Vec<tor_observability::services::trace::RequestTrace> {
    observability::snapshot_traces()
}

/// Activates numeric-only SQLx route metrics; must run once before opening PG.
/// If the process already uses another subscriber, RIL SQL metrics stay OFF.
pub fn activate_ril_sqlx_observation() -> bool {
    ril::activate_sqlx_observation()
}

pub fn start_ril_pg_background(catalog: &PgCatalog) {
    ril::start_pg_background(tor_traffic_behavior::repositories::PgRilRollups::new(
        catalog.pool(),
    ));
}

/// Instrument late-bound S023 routes without adding another fallback or
/// duplicating observations from the already-instrumented base router.
pub fn instrument_additional_routes(router: Router) -> Router {
    ril::instrument_additional(router)
}
pub fn router(repo: Arc<dyn PublicBookRepository>) -> Router {
    ril::instrument(make_router(repo, None, None, false, None, None))
}
pub fn router_with_postgres(catalog: PgCatalog) -> Router {
    ril::instrument(make_router(
        Arc::new(catalog.clone()),
        Some(catalog),
        None,
        false,
        None,
        None,
    ))
}
pub fn router_with_postgres_reading(catalog: PgCatalog, reader: PgReader) -> Router {
    ril::instrument(make_router(
        Arc::new(catalog.clone()),
        Some(catalog),
        Some(reader),
        false,
        None,
        None,
    ))
}

pub fn router_with_postgres_reading_auth(
    catalog: PgCatalog,
    reader: PgReader,
    accounts: tor_accounts::repositories::PgAccounts,
    secure: bool,
    public_origin: String,
) -> Router {
    admin_reader_router::authenticated_postgres_reader_without_paid_g(
        catalog,
        reader,
        accounts,
        secure,
        public_origin,
        None,
        false,
    )
}

fn make_router(
    repo: Arc<dyn PublicBookRepository>,
    postgres: Option<PgCatalog>,
    reader: Option<PgReader>,
    active_auth: bool,
    objects: Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>>,
    follow_control: Option<book_follow_page::FollowBookControl>,
) -> Router {
    make_router_with_paid_checkout(
        repo,
        postgres,
        reader,
        active_auth,
        objects,
        follow_control,
        ReaderModes {
            paid_browser: false,
            admin_reader: false,
        },
    )
}

fn make_router_with_paid_checkout(
    repo: Arc<dyn PublicBookRepository>,
    postgres: Option<PgCatalog>,
    reader: Option<PgReader>,
    active_auth: bool,
    objects: Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>>,
    follow_control: Option<book_follow_page::FollowBookControl>,
    modes: ReaderModes,
) -> Router {
    let api = tor_books::api::router(repo.clone());
    // Only real authenticated PG routers may capture referral book query links.
    // This layer runs *after* book_detail has accepted public visibility.
    let book_router = Router::new().route("/books/{id}", get(book_detail));
    let book_router = if let Some(control) = follow_control.clone() {
        book_router.route_layer(axum::middleware::from_fn_with_state(
            control,
            referral_book_http::capture,
        ))
    } else {
        book_router
    };
    // Only the four public HTML routes get A-owned optional consent
    // verification. The account/session lookup is disabled by default and
    // shares the same signer, opt-in and single-instance safety gates as RIL.
    let public_pages = Router::new()
        .route("/", get(homepage))
        .route("/catalog", get(catalog_http::catalog))
        .merge(book_router)
        .route("/reader/{id}", get(reader_page));
    let public_pages = if let Some(control) = follow_control.as_ref() {
        let (accounts, cookies) = control.browser_context();
        let context = auth::behavior_consent::PublicFlowConsentContext::new(
            accounts.clone(),
            cookies.clone(),
        );
        if context.enabled() {
            public_pages.layer(axum::middleware::from_fn_with_state(
                context,
                auth::behavior_consent::annotate_public_flow,
            ))
        } else {
            public_pages
        }
    } else {
        public_pages
    };
    let mut app = Router::new()
        .merge(public_pages)
        .route("/covers/{id}/{variant}", get(cover_http::cover_image))
        .route(
            "/api/v1/covers/{id}/{variant}/direct-grant",
            get(cover_http::cover_direct_grant),
        )
        .route("/api/v1/books/{id}/toc", get(book_toc))
        .route("/api/v1/chapters/{id}/read", get(denied_reader_api))
        .route("/assets/newboo-text-issues.js", get(text_issue_script))
        .route(
            "/assets/newboo-free-reader.js",
            get(pages::free_reader_script),
        )
        .route("/assets/site.css", get(pages::stylesheet))
        .route("/assets/newboo.css", get(pages::newboo_stylesheet))
        .route(
            "/assets/newboo-paid-reader.mjs",
            get(pages::paid_reader_script),
        )
        .route(
            "/assets/newboo-paid-comments.mjs",
            get(pages::paid_comments_script),
        )
        .route(
            "/assets/newboo-layer-reader.mjs",
            get(layer_reader_assets::script),
        )
        .route(
            "/assets/newboo-cover-picker.js",
            get(pages::cover_picker_script),
        )
        .route(
            "/assets/newboo-chapter-import.js",
            get(chapter_import_assets::script),
        )
        .route(
            "/assets/brand/pandaolate-header.png",
            get(pages::brand_logo),
        )
        .route(
            "/assets/brand/pandaolate-header-dark.png",
            get(pages::brand_logo_dark),
        )
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/metrics", get(observability::metrics_endpoint))
        .route("/ops/traces", get(operator_traces::page))
        .route("/ops/overview", get(operator_overview::page));
    if !active_auth {
        app = app
            .route("/login", get(login))
            .route("/register", get(register));
    }
    let search = postgres
        .as_ref()
        .map(|catalog| tor_search::PgSearch::new(catalog.pool()));
    let search_api = search
        .clone()
        .map(tor_search::api::router)
        .unwrap_or_default();
    let covers = postgres
        .as_ref()
        .zip(objects)
        .map(|(catalog, store)| cover_http::configured_delivery(catalog, store));
    let rec_runtime = follow_control.as_ref().map(|control| {
        let (accounts, cookies) = control.browser_context();
        recommendation_http::Runtime::new(accounts.clone(), cookies.clone())
    });
    let app = app
        .with_state(AppState {
            books: repo,
            postgres,
            reader,
            search,
            paid_checkout_browser: active_auth
                && modes.paid_browser
                && std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref() == Ok("1"),
            // Administrator private reading is independent of paid checkout.
            private_admin_reader_enabled: active_auth && modes.admin_reader,
            private_admin_sealed_reader_enabled: active_auth
                && modes.admin_reader
                && modes.paid_browser,
            paid_batch_browser_enabled: active_auth
                && std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref() == Ok("1"),
            covers,
            follow_control,
        })
        .merge(api)
        .merge(search_api);
    recommendation_http::instrument(app, rec_runtime)
}
#[cfg(test)]
mod tests;
