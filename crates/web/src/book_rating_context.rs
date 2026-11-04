//! S020: Book detail's age-scoped ratings and like counter.
//! A verified adult/pro_adult reader must not be downgraded by the
//! anonymous basic-only rating projections after B grants direct Book access.
use axum::http::StatusCode;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgBookLikes;
use tor_recommendations::repositories::{BookStars, PgBookStars};

pub(super) async fn for_book(
    catalog: &PgCatalog,
    book_id: i64,
    session_digest: Option<&[u8; 32]>,
) -> Result<Option<(BookStars, i64)>, StatusCode> {
    // Re-resolve server-side A session, never a browser supplied user/tier.
    let actor = if let Some(session) = session_digest {
        sqlx::query_scalar::<_, i64>(
            "SELECT s.user_id FROM sessions s
             JOIN users u ON u.id=s.user_id AND u.account_state='active'
             WHERE s.token_hash=$1::bytea AND s.revoked_at IS NULL
               AND s.expires_at>now()",
        )
        .bind(session.as_slice())
        .fetch_optional(catalog.pool())
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    } else {
        None
    };

    let stars = PgBookStars::new(catalog.pool().clone());
    let summary = match actor {
        Some(actor) => stars.visible_summary_for_actor(actor, book_id).await,
        None => stars.visible_summary(book_id).await,
    }
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Some(summary) = summary else {
        return Ok(None);
    };

    let likes = PgBookLikes::new(catalog.pool().clone());
    let count = match actor {
        Some(actor) => likes
            .visible_summary(actor, book_id)
            .await
            .map(|summary| summary.map(|summary| summary.count)),
        None => likes.public_count(book_id).await,
    }
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(count.map(|count| (summary, count)))
}
