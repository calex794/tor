//! Authenticated, bounded public title suggestions on the new-book form.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HintQuery {
    language: String,
    title: Option<String>,
    original: Option<String>,
    alt1: Option<String>,
    alt2: Option<String>,
    alt3: Option<String>,
}
#[derive(Serialize)]
struct Item {
    id: i64,
    title: String,
    language: String,
}
#[derive(Serialize)]
struct Results {
    items: Vec<Item>,
}

pub(super) async fn find(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<HintQuery>,
) -> Response {
    match session_user(&state, &headers).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let names = [
        query.title.as_deref().unwrap_or(""),
        query.original.as_deref().unwrap_or(""),
        query.alt1.as_deref().unwrap_or(""),
        query.alt2.as_deref().unwrap_or(""),
        query.alt3.as_deref().unwrap_or(""),
    ];
    match state
        .authoring
        .public_title_suggestions(&query.language, &names)
        .await
    {
        Ok(items) => private_response(
            Json(Results {
                items: items
                    .into_iter()
                    .map(|item| Item {
                        id: item.id,
                        title: item.title,
                        language: item.language,
                    })
                    .collect(),
            }),
            &[],
        ),
        Err(tor_books::services::authoring::AuthorError::Invalid) => {
            deny(StatusCode::BAD_REQUEST)
        }
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn script() -> Response {
    (
        [("content-type", "text/javascript; charset=utf-8"),
         ("cache-control", "public, max-age=300"),
         ("x-content-type-options", "nosniff")],
        include_str!("../../../../assets/book_title_suggestions.js"),
    ).into_response()
}
