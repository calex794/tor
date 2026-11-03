//! S031: owner-only explicit price publication; never an automatic charge.
use super::{deny, layer_books, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::post,
    Router,
};
use serde::Deserialize;
use tor_commerce::{repositories::layers::PgLayers, services::layers::LayerService};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OfferForm {
    expected_version_id: i64,
    expected_offer_id: i64,
    price_rub: String,
    term_kind: String,
    csrf_token: Option<String>,
}

pub(super) fn routes() -> Router<AuthState> {
    Router::new().route("/me/layers/{layer_id}/offer", post(publish))
}

/// Exact decimal RUB, accepting Russian comma or decimal point. Never floats.
fn exact_minor(raw: &str) -> Option<i64> {
    let input = raw.trim();
    if input.is_empty() || input.len() > 18 {
        return None;
    }
    let (whole, fraction) = match input.split_once([',', '.']) {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (input, None),
    };
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?;
    let kopecks = match fraction {
        None => 0,
        Some(part) if part.len() == 1 && part.bytes().all(|b| b.is_ascii_digit()) => {
            part.parse::<i64>().ok()?.checked_mul(10)?
        }
        Some(part) if part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()) => {
            part.parse::<i64>().ok()?
        }
        _ => return None,
    };
    let amount = whole.checked_mul(100)?.checked_add(kopecks)?;
    (amount > 0).then_some(amount)
}

pub(super) async fn publish(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<OfferForm>,
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
    let (Ok(actor), Ok(layer), Ok(expected_version), Some(price_minor)) = (
        PositiveId::new(actor),
        PositiveId::new(raw_layer),
        PositiveId::new(form.expected_version_id),
        exact_minor(&form.price_rub),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let expected_offer = if form.expected_offer_id == 0 {
        None
    } else {
        match PositiveId::new(form.expected_offer_id) {
            Ok(offer) => Some(offer),
            Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        }
    };
    let lifetime = match form.term_kind.as_str() {
        "prepaid_30d" => false,
        "lifetime" => true,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service
        .offer_if_current(
            actor,
            layer,
            expected_version,
            expected_offer,
            price_minor,
            lifetime,
        )
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/me/layers/{raw_layer}/books")), &[]),
        Err(error) => deny(layer_books::status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_05_owner_offer_price_is_exact_rub_kopecks() {
        for (raw, expected) in [
            ("149", Some(14900)),
            ("0,01", Some(1)),
            ("149,99", Some(14999)),
            ("149.9", Some(14990)),
            ("  1,00  ", Some(100)),
            ("0", None),
            ("0,00", None),
            ("-1", None),
            ("+1", None),
            ("1.001", None),
            ("1,2,3", None),
            ("1.", None),
            (".10", None),
            ("NaN", None),
            ("92233720368547758,08", None),
        ] {
            assert_eq!(exact_minor(raw), expected, "bad exact RUB price {raw:?}");
        }
    }
}
