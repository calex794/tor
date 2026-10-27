//! Account route assembly; keep auth.rs below the source-size limit.
use super::*;

pub(crate) fn router(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
) -> Router {
    router_inner(accounts, reader, secure, origin, None, false, None)
}

pub(crate) fn router_with_cover_uploads(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    storage: Arc<dyn CoverObjectStore>,
) -> Router {
    router_inner(accounts, reader, secure, origin, Some(storage), false, None)
}

/// Enable purchased-library navigation only when an authorized reader is mounted.
pub(crate) fn router_with_paid_reader(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover_upload_storage: Option<Arc<dyn CoverObjectStore>>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover_upload_storage,
        true,
        None,
    )
}

/// Only the private, explicitly enabled paid-author router provides a writer.
/// The existing grant-only constructor never changes author capabilities.
pub(crate) fn router_with_paid_reader_author(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover_upload_storage: Option<Arc<dyn CoverObjectStore>>,
    author: Arc<dyn author_chapter_http::PaidChapterPublisher>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover_upload_storage,
        true,
        Some(author),
    )
}

fn router_inner(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    storage: Option<Arc<dyn CoverObjectStore>>,
    paid_reader_available: bool,
    paid_author: Option<Arc<dyn author_chapter_http::PaidChapterPublisher>>,
) -> Router {
    let cookies = CookiePolicy { secure, origin };
    let chapter_routes = author_chapter_http::router(accounts.clone(), cookies.clone(), paid_author);
    let cover_routes = storage
        .as_ref()
        .map(|store| author_cover_http::router(accounts.clone(), cookies.clone(), store.clone()));
    let progress = PgReadingProgress::new(accounts.pool().clone());
    let sync = PgReadingSync::new(accounts.pool().clone());
    let bookmarks = PgLibrary::new(accounts.pool().clone());
    let rankings = tor_recommendations::repositories::PgRanking::new(accounts.pool().clone());
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let checkout = tor_commerce::repositories::checkout::PgCheckout::new(accounts.pool().clone());
    let paid_checkout_enabled = std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref() == Ok("1");
    let cover_jobs = storage
        .as_ref()
        .map(|_| PgCoverJobs::new(accounts.pool().clone()));
    let cover_upload_enabled = storage.is_some();
    let state = AuthState {
        accounts,
        checkout,
        paid_checkout_enabled,
        paid_reader_available,
        authoring,
        progress,
        sync,
        bookmarks,
        reader,
        rankings,
        cookies,
        password_jobs: PasswordJobs::new(4),
        ril_signer: behavior_consent::configured_ril_signer(),
        cover_jobs,
        cover_upload_enabled,
    };
    let observation_state = state.clone();
    let app = Router::new()
        .route(
            "/register",
            get(account_form_http::show_registration).post(account_form_http::register),
        )
        .route(
            "/login",
            get(account_form_http::show_login).post(account_form_http::login),
        )
        .route("/me", get(me))
        .route("/me/library", get(my_library))
        .route("/me/rankings", get(rankings::page))
        .route("/me/rankings/choice", post(rankings::browser_choice))
        .route("/me/rankings/resume", post(rankings::browser_resume))
        .route("/api/v1/me/rankings", get(rankings::api_state))
        .route("/api/v1/me/rankings/choice", post(rankings::api_choice))
        .route("/api/v1/me/rankings/resume", post(rankings::api_resume))
        .route("/api/v1/purchases", post(checkout::buy))
        .route(
            "/api/v1/me/progress/free",
            get(sync_actions::get_free).put(sync_actions::put_free),
        )
        .route(
            "/api/v1/me/progress/purchased",
            get(sync_actions::get_purchased).put(sync_actions::put_purchased),
        )
        .route(
            "/api/v1/me/progress/purchased/complete",
            get(paid_completion::get).post(paid_completion::post),
        )
        .route("/me/library/add/{id}", get(library_actions::show_add))
        .route("/me/library/{id}/save", post(library_actions::browser_add))
        .route(
            "/me/library/{id}/remove",
            post(library_actions::browser_remove),
        )
        .route("/api/v1/me/library", get(library_actions::api_list))
        .route(
            "/api/v1/me/library/{id}",
            post(library_actions::api_add).delete(library_actions::api_delete),
        )
        .route("/me/books", get(author_books).post(create_book))
        .route("/me/books/new", get(new_author_book))
        .route("/me/books/{id}", get(author_book))
        .route("/me/books/{id}/settings/covers", get(cover_settings))
        .route("/read/{id}", get(private_reader))
        .route("/read/{id}/place", post(save_place))
        .route("/read/{id}/complete", post(complete_chapter::complete))
        .route("/logout", post(logout))
        .route("/me/behavior", post(behavior_consent::set_behavior_consent))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            observation_state,
            behavior_consent::annotate_verified_ril,
        ))
        .merge(chapter_routes);
    if let Some(cover_routes) = cover_routes {
        app.merge(cover_routes)
    } else {
        app
    }
}

