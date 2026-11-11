//! Current public-book aggregates from existing C facts, with fresh A authority.
use super::{administrator, secure, AudienceState};
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};
use tor_reading::services::audience::{
    canonical::{CanonicalDimension, CanonicalQuery},
    AudienceError,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadingQuery {
    days: Option<i32>,
    kind: Option<String>,
    genre: Option<String>,
    tag: Option<String>,
    without_genre: Option<bool>,
    without_tag: Option<bool>,
    after_id: Option<i64>,
    after_label: Option<String>,
    view: Option<String>,
}
impl ReadingQuery {
    fn validated(self) -> Result<(CanonicalQuery, bool), AudienceError> {
        let ranked = match self.view.as_deref().unwrap_or("all") {
            "all" => false,
            "top20" => true,
            _ => return Err(AudienceError::Invalid),
        };
        if ranked && (self.after_id.is_some() || self.after_label.is_some()) {
            return Err(AudienceError::Invalid);
        }
        let query = CanonicalQuery {
            days: self.days.unwrap_or(7),
            dimension: CanonicalDimension::parse(self.kind.as_deref().unwrap_or("books"))?,
            genre: self.genre,
            tag: self.tag,
            without_genre: self.without_genre.unwrap_or(false),
            without_tag: self.without_tag.unwrap_or(false),
            after_id: self.after_id,
            after_label: self.after_label,
        }
        .validated()?;
        Ok((query, ranked))
    }
}
pub(super) async fn page(
    State(state): State<AudienceState>,
    headers: HeaderMap,
    query: Result<Query<ReadingQuery>, QueryRejection>,
) -> Response {
    let deadline = Instant::now() + Duration::from_millis(950);
    match timeout_at(deadline, administrator(&state, &headers)).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => return secure(status),
        Err(_) => return secure(StatusCode::SERVICE_UNAVAILABLE),
    }
    let (query, ranked) = match query
        .map_err(|_| AudienceError::Invalid)
        .and_then(|Query(q)| q.validated())
    {
        Ok(q) => q,
        Err(_) => return secure(StatusCode::BAD_REQUEST),
    };
    if ranked {
        let report = match timeout_at(deadline, state.store.canonical_rank_report(&query)).await {
            Ok(Ok(report)) => Some(report),
            Ok(Err(AudienceError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
            _ => None,
        };
        return secure((
            if report.is_some() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE },
            super::render::canonical::ranked_page(&query, report.as_ref()),
        ));
    }
    let report = match timeout_at(deadline, state.store.canonical_report(&query)).await {
        Ok(Ok(report)) => Some(report),
        Ok(Err(AudienceError::Invalid)) => return secure(StatusCode::BAD_REQUEST),
        _ => None,
    };
    secure((
        if report.is_some() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        super::render::canonical::page(&query, report.as_ref()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_uses_the_whole_filter_and_cannot_accept_a_page_cursor() {
        for raw in [
            "view=unknown", "view=top20&after_id=1",
            "view=top20&kind=tags&after_label=a", "view=top20&view=all",
        ] {
            let uri = format!("/admin/audience/reading?{raw}").parse().unwrap();
            assert!(Query::<ReadingQuery>::try_from_uri(&uri)
                .map_err(|_| AudienceError::Invalid)
                .and_then(|Query(q)| q.validated()).is_err(), "{raw}");
        }
        for (raw, ranked) in [
            ("days=7&after_id=20", false),
            ("days=7&view=all&after_id=20", false),
            ("days=7&view=top20&kind=genres&genre=Fantasy", true),
        ] {
            let uri = format!("/admin/audience/reading?{raw}").parse().unwrap();
            let Query(query) = Query::<ReadingQuery>::try_from_uri(&uri).unwrap();
            let (query, actual) = query.validated().unwrap();
            assert_eq!(actual, ranked);
            assert_eq!(query.days, 7);
        }
    }
}
