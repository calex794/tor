//! D-05: private owner-only Layer creation; no checkout or money posting.
use super::{deny, layer_books, session_user, AuthState};
use crate::{auth_cookies::private_response, author_pages, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::money::IdempotencyKey, repositories::layers::PgLayers, services::layers::LayerService,
};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateForm {
    title: String,
    command_key: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MetadataForm {
    title: String,
    description: String,
    status: String,
    expected_revision: i64,
    csrf_token: Option<String>,
}

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/me/layers", get(index))
        .route("/me/layers/new", get(page).post(create))
        .route("/me/layers/{layer_id}/metadata", axum::routing::post(update_metadata))
}

/// Owner-only entry for S032; no payment, buyer data or public enumeration.
pub(super) async fn index(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !layer_books::enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    };
    let actor = match PositiveId::new(actor) {
        Ok(actor) => actor,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service.owned_layers(actor, 100).await {
        Ok(layers) => {
            let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
            private_response(author_pages::layers_index_with_forms(&layers, &csrf), &cookies)
        }
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

/// Status transitions use a separately versioned owner-only metadata CAS.
pub(super) async fn update_metadata(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<MetadataForm>,
) -> Response {
    if !layer_books::enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Ok(actor), Ok(layer)) = (PositiveId::new(actor), PositiveId::new(raw_layer)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service
        .update_metadata_if_current(
            actor,
            layer,
            form.expected_revision,
            &form.title,
            &form.description,
            &form.status,
        )
        .await
    {
        Ok(()) => private_response(Redirect::to("/me/layers"), &[]),
        Err(error) => deny(layer_books::status(error)),
    }
}

fn new_layer_html(csrf: &str, key: &str) -> axum::response::Html<String> {
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card">
        <p class="eyebrow">Кабинет автора</p><h1>Создать Layer</h1>
        <p>Создаётся пустой Layer. Книги добавляются отдельно, только когда
        в команде один активный участник — владелец.</p>
        <form method="post" action="/me/layers/new">
        <label for="layer-title">Название Layer</label>
        <input id="layer-title" name="title" type="text" maxlength="120" required>
        <input type="hidden" name="csrf_token" value="{}">
        <input type="hidden" name="command_key" value="{}">
        <button class="button" type="submit">Создать Layer</button>
        </form>
        <p><a href="/me/layers">Мои слои</a> · <a href="/me/books">Мои книги</a></p>
        </div></div></section>"#,
        pages::escape_html(csrf),
        pages::escape_html(key)
    );
    pages::layout(
        "Создать Layer",
        "Создание собственного Layer без покупки.",
        &content,
    )
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !layer_books::enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    match session_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    // Every form gets a cryptographically random, canonical 32-byte token.
    let key = generate_session_token();
    private_response(new_layer_html(&csrf, &key), &cookies)
}

pub(super) async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    if !layer_books::enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Ok(actor), Ok(key)) = (
        PositiveId::new(actor),
        IdempotencyKey::parse(&form.command_key),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service.create_once(actor, &form.title, &key).await {
        Ok(receipt) => private_response(
            Redirect::to(&format!("/me/layers/{}/books", receipt.layer_id)),
            &[],
        ),
        Err(error) => deny(layer_books::status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_05_create_form_has_no_client_actor_and_escapes_all_tokens() {
        let html = new_layer_html("<csrf>", "\"><script>x</script>").0;
        assert!(html.contains("name=\"title\""));
        assert!(html.contains("name=\"command_key\""));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(html.contains("&lt;csrf&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("user_id"));
        assert!(!html.contains("price_minor"));
        assert!(html.contains("maxlength=\"120\""));
    }
}
