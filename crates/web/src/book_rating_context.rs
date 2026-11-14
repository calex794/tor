//! S020: Book detail's server-scoped rating and like counter.
//! The canonical B reader grants access before this projection; anonymous
//! basic-only metadata must never downgrade an eligible adult/pro_adult reader.
use axum::http::StatusCode;
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_recommendations::repositories::{BookStars, PgBookStars};

pub(super) async fn for_book(
    catalog: &PgCatalog,
    book_id: i64,
    session_digest: Option<&[u8; 32]>,
) -> Result<Option<(BookStars, i64)>, StatusCode> {
    // Bind the actual A session to the server-side actor, not a client id.
    let actor = if let Some(session) = session_digest {
        PgAccounts::from_pool(catalog.pool().clone())
            .active_session_user(session)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    } else {
        None
    };
    // B/C/F display gates agree for this direct Book route, including the
    // already-readable pending_deletion case. Both aggregates must come from
    // one current visibility/age snapshot, never from a cached session.
    PgBookStars::new(catalog.pool().clone())
        .book_page_summary_for_actor(book_id, actor)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
