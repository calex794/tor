//! S020: Book detail's server-scoped rating and like counter.
//! The canonical B reader grants access before this projection; anonymous
//! basic-only metadata must never downgrade an eligible adult/pro_adult reader.
use axum::http::StatusCode;
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgBookLikes;
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
    let ratings = PgBookStars::new(catalog.pool().clone());
    let summary = ratings
        .visible_summary_for_actor(book_id, actor)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Some(summary) = summary else {
        return Ok(None);
    };

    let likes = PgBookLikes::new(catalog.pool().clone());
    // The already-authorized direct Book page may remain visible during
    // pending_deletion even though catalog, public-like pages and mutations
    // must unlist it. Independently recheck the live Book and actor age gate.
    let count = match actor {
        Some(actor) => likes
            .direct_view_summary(actor, book_id)
            .await
            .map(|summary| summary.map(|summary| summary.count)),
        None => likes.direct_view_count(book_id).await,
    }
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(count.map(|count| (summary, count)))
}
