use crate::{
    schemas::BookView,
    services::{CatalogError, ListPublicBooks, PublicBookRepository, DEFAULT_PAGE_SIZE},
};
use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use crate::services::import_preview::{editable_plain_text, validate_editable_preview};
use std::sync::Arc;

#[derive(Clone)]
struct ApiState {
    repository: Arc<dyn PublicBookRepository>,
}
#[derive(Deserialize)]
struct CatalogQuery {
    limit: Option<usize>,
}

async fn list_books(
    State(state): State<ApiState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<Vec<BookView>>, StatusCode> {
    let books = ListPublicBooks::new(state.repository.as_ref())
        .execute(query.limit.unwrap_or(DEFAULT_PAGE_SIZE))
        .await
        .map_err(|e| match e {
            CatalogError::InvalidLimit => StatusCode::BAD_REQUEST,
            CatalogError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        })?;
    Ok(Json(
        books
            .into_iter()
            .map(|book| BookView {
                id: book.id(),
                title: book.title().to_owned(),
                target_language: book.key().target_language().to_owned(),
            })
            .collect(),
    ))
}
#[derive(Deserialize)]
struct ChapterPreviewRequest {
    text: String,
    #[serde(default)]
    merge_at_lines: Vec<usize>,
}

#[derive(Serialize)]
struct ChapterPreviewItem {
    title: String,
    body: String,
}

#[derive(Serialize)]
struct ChapterPreviewResponse {
    chapters: Vec<ChapterPreviewItem>,
    detected_boundaries: Vec<usize>,
}

/// Stateless preview only: never creates or changes a chapter.
async fn preview_chapter_import(
    Json(request): Json<ChapterPreviewRequest>,
) -> Result<Json<ChapterPreviewResponse>, StatusCode> {
    // Bound the payload before normalization and parsing.
    if request.text.len() > 2_000_000 || request.merge_at_lines.len() > 500 {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let mut preview = editable_plain_text(&request.text)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let detected_boundaries = preview.boundaries().iter().map(|b| b.line).collect();
    for line in request.merge_at_lines {
        preview.merge_false_boundary(line)
            .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    }
    let chapters = validate_editable_preview(&preview)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?
        .into_iter()
        .map(|chapter| ChapterPreviewItem { title: chapter.title, body: chapter.body })
        .collect();
    Ok(Json(ChapterPreviewResponse { chapters, detected_boundaries }))
}

pub fn router(repository: Arc<dyn PublicBookRepository>) -> Router {
    Router::new()
        .route("/api/v1/books", get(list_books))
        .route(
            "/api/v1/books/import-preview",
            post(preview_chapter_import).layer(DefaultBodyLimit::max(2_000_000)),
        )
        .with_state(ApiState { repository })
}

#[cfg(test)]
mod import_preview_http_tests {
    use super::*;
    use axum::{body::to_bytes, http::Request, routing::post};
    use tower::ServiceExt;

    #[tokio::test]
    async fn merges_false_boundary_without_publishing() {
        let app = Router::new().route("/preview", post(preview_chapter_import));
        let request = Request::builder()
            .method("POST")
            .uri("/preview")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({
                    "text": "Глава 1: Начало\\nТекст\\nГлава 2: Ложная\\nПродолжение\\nГлава 3: Финал\\nКонец",
                    "merge_at_lines": [2]
                }).to_string()
            )).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["chapters"].as_array().unwrap().len(), 2);
        assert!(value["chapters"][0]["body"].as_str().unwrap().contains("Глава 2: Ложная"));
    }

    #[tokio::test]
    async fn rejects_invalid_boundary() {
        let app = Router::new().route("/preview", post(preview_chapter_import));
        let request = Request::builder().method("POST").uri("/preview")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::json!({"text":"Глава 1: Начало\\nТекст","merge_at_lines":[99]}).to_string()
            )).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
