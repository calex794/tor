//! D-05 reader-facing Layer pages. No alternate right to protected chapters.
use super::{deny, layer_checkout_api, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{rejection::FormRejection, Form, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{domain::layers::LayerError, repositories::layer_storefront};

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/me/library/layers", get(my_terms))
}

#[derive(Default, Deserialize)]
pub(super) struct PageQuery {
    purchased: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PurchaseForm {
    csrf_token: String,
    idempotency_key: String,
    expected_offer_id: String,
    expected_version_id: String,
    expected_price_minor: i64,
    confirm: Option<String>,
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(layer_id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    if !state.layer_pages_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let layer = match layer_checkout_api::positive(&layer_id) {
        Ok(layer) => layer,
        Err(_) => return deny(StatusCode::NOT_FOUND),
    };
    let actor = match session_user(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    let offer = match layer_storefront::storefront(state.accounts.pool(), layer.get(), actor).await
    {
        Ok(Some(offer)) => offer,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let should_offer = state.layer_checkout_enabled
        && actor.is_some()
        && actor != Some(offer.owner_id)
        && offer.public_book_count > 0
        && !offer.has_lifetime_access;
    let (csrf, cookies) = if should_offer {
        let (token, cookies) = state.cookies.csrf_for_page(&headers);
        (Some(token), cookies)
    } else {
        (None, Vec::new())
    };
    let key = should_offer.then(generate_session_token);
    let purchased = query.purchased.as_deref() == Some("1") && offer.has_access;
    private_response(
        pages::layer_offer_page(
            &offer,
            actor,
            csrf.as_deref(),
            key.as_deref(),
            purchased,
            state.layer_checkout_enabled,
        ),
        &cookies,
    )
}

async fn my_terms(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !state.layer_pages_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match layer_storefront::purchased_terms(actor, state.accounts.pool()).await {
        Ok(terms) => private_response(
            pages::purchased_layers_page(&terms, state.paid_reader_available),
            &[],
        ),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn failure(error: LayerError, layer: i64) -> Response {
    let (title, message) = match error {
        LayerError::InsufficientFunds => (
            "Недостаточно средств",
            "Пополните кошелёк и повторите покупку.",
        ),
        LayerError::StaleOffer => (
            "Условия пакета изменились",
            "Проверьте состав и цену на странице пакета перед покупкой.",
        ),
        LayerError::ReplayConflict => (
            "Повтор покупки отклонён",
            "Другая операция уже использовала этот ключ покупки. Обновите страницу.",
        ),
        LayerError::Inaccessible => ("Пакет недоступен", "Этот пакет сейчас нельзя приобрести."),
        LayerError::Unavailable => (
            "Покупка временно недоступна",
            "Проверьте историю покупок перед повторной попыткой.",
        ),
        LayerError::Invalid => (
            "Проверьте условия покупки",
            "Форма покупки устарела или заполнена неверно. Обновите страницу.",
        ),
    };
    private_response(
        (
            layer_checkout_api::layer_status(error),
            pages::layer_purchase_error(title, message, layer),
        ),
        &[],
    )
}

pub(super) async fn buy_form(
    State(state): State<AuthState>,
    Path(layer_id): Path<String>,
    headers: HeaderMap,
    form: Result<Form<PurchaseForm>, FormRejection>,
) -> Response {
    if !state.layer_pages_enabled || !state.layer_checkout_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let layer = match layer_checkout_api::positive(&layer_id) {
        Ok(layer) => layer,
        Err(_) => return deny(StatusCode::NOT_FOUND),
    };
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    // Native HTML POSTs are money-changing operations: require explicit
    // trusted Origin, checked CSRF cookie and a human-confirmed exact price.
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let form = match form {
        Ok(Form(form)) => form,
        Err(_) => return failure(LayerError::Invalid, layer.get()),
    };
    if !state.cookies.valid_csrf(&headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    if form.confirm.as_deref() != Some("1") {
        return failure(LayerError::Invalid, layer.get());
    }
    match layer_checkout_api::charge(
        &state,
        actor,
        &layer_id,
        &form.expected_offer_id,
        &form.expected_version_id,
        form.expected_price_minor,
        &form.idempotency_key,
    )
    .await
    {
        Ok(_) => private_response(
            Redirect::to(&format!("/layers/{layer_id}?purchased=1")),
            &[],
        ),
        Err(error) => failure(error, layer.get()),
    }
}
