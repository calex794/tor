//! F-04 homepage orchestration: new additions and real free chapter releases.
//! This is a read-only public projection. B remains the only publisher.
use super::{cover_http, pages, public_cards, AppState};
use axum::{http::StatusCode, response::Html};
use std::collections::HashSet;
use tor_search::{
    domain::SearchBook,
    repositories::showcases::PgShowcases,
    services::showcases::{RecentShowcases, ReleaseShowcases, RisingShowcases},
    SearchError,
};

fn status(error: SearchError) -> StatusCode {
    match error {
        SearchError::Invalid => StatusCode::BAD_REQUEST,
        SearchError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn card(book: SearchBook) -> pages::PublicBookCard {
    pages::PublicBookCard {
        id: book.id,
        title: book.title,
        language: book.language,
        author_name: (!book.author_name.is_empty()).then_some(book.author_name),
        genres: book.genres,
    }
}

pub(super) async fn homepage(
    state: AppState,
    issuer: Option<std::sync::Arc<dyn tor_contracts::exposure::ObservationIssuer>>,
) -> Result<Html<String>, StatusCode> {
    let Some(catalog) = state.postgres.as_ref() else {
        return Ok(pages::home(&public_cards(state.books.as_ref(), 6).await?));
    };
    let repository = PgShowcases::new(catalog.pool());
    let latest = RecentShowcases::new(&repository)
        .latest_added(None, 12)
        .await
        .map_err(status)?
        .into_iter()
        .map(card)
        .collect::<Vec<_>>();
    let releases = ReleaseShowcases::new(&repository)
        .recent_free_releases(12)
        .await
        .map_err(status)?
        .into_iter()
        .map(card)
        .collect::<Vec<_>>();

    // Opt in only after a real-data PostgreSQL timing and abuse review.
    let rising = if std::env::var("TOR_RISING_SHELF_ENABLED").as_deref() == Ok("1") {
        RisingShowcases::new(&repository)
            .rising_free_readers(12)
            .await
            .map_err(status)?
            .into_iter()
            .map(card)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let mut cover_candidates = latest.clone();
    let mut ids = cover_candidates
        .iter()
        .map(|book| book.id)
        .collect::<HashSet<_>>();
    for book in &releases {
        if ids.insert(book.id) {
            cover_candidates.push(book.clone());
        }
    }
    for book in &rising {
        if ids.insert(book.id) {
            cover_candidates.push(book.clone());
        }
    }
    let covers = cover_http::cover_card_ids(state.covers.as_ref(), &cover_candidates).await?;
    cover_http::homepage_with_cdn(
        &state,
        &latest,
        &releases,
        &rising,
        &covers,
        issuer.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_preserves_canonical_public_book_id() {
        let mapped = card(SearchBook {
            id: 77,
            title: "Глава вышла".into(),
            language: "ru".into(),
            author_name: String::new(),
            genres: vec!["Система".into()],
            tags: Vec::new(),
            fandoms: Vec::new(),
        });
        assert_eq!(mapped.id, 77);
        assert_eq!(mapped.author_name, None);
        assert_eq!(mapped.genres, vec!["Система"]);
    }
}
