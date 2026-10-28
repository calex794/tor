//! D-05: opt-in internal-wallet Layer checkout. No external payment provider.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use tor_commerce::{
    domain::{
        layers::{LayerCheckoutCommand, LayerError},
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuyLayer {
    expected_offer_id: String,
    expected_version_id: String,
    expected_price_minor: i64,
}

#[derive(Serialize)]
struct LayerPurchaseReceipt {
    term_id: String,
    balance_minor: i64,
    replayed: bool,
}

fn positive(raw: &str) -> Result<PositiveId, StatusCode> {
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    raw.parse::<i64>()
        .ok()
        .and_then(|id| PositiveId::new(id).ok())
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)
}

fn layer_status(error: LayerError) -> StatusCode {
    match error {
        LayerError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        LayerError::Inaccessible => StatusCode::NOT_FOUND,
        LayerError::StaleOffer | LayerError::InsufficientFunds | LayerError::ReplayConflict => {
            StatusCode::CONFLICT
        }
        LayerError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Explicit operator opt-in for Layer charges, independent of chapter checkout.
pub(super) fn configured() -> bool {
    std::env::var("TOR_LAYER_CHECKOUT_ENABLED").as_deref() == Ok("1")
}

pub(super) async fn buy(
    State(state): State<AuthState>,
    Path(layer_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<BuyLayer>,
) -> Response {
    // Never inherit the ordinary chapter checkout switch: Layer financial
    // settlement and user-facing contracts are a separate operator opt-in.
    if !state.layer_checkout_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    // Money-changing JSON requests require an explicit trusted browser Origin.
    // A valid CSRF cookie alone is not sufficient for this endpoint.
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
        != Some(state.cookies.origin.as_str())
        || !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|token| state.cookies.valid_csrf(&headers, token))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let key = match headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| IdempotencyKey::parse(raw).ok())
    {
        Some(key) => key,
        None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let (actor, layer, offer, version) = match (
        PositiveId::new(actor).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY),
        positive(&layer_id),
        positive(&input.expected_offer_id),
        positive(&input.expected_version_id),
    ) {
        (Ok(actor), Ok(layer), Ok(offer), Ok(version)) => (actor, layer, offer, version),
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let price = match MoneyMinor::new(input.expected_price_minor) {
        Ok(price) if price.get() > 0 => price,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let wallet = match WalletId::new(actor.get()) {
        Ok(wallet) => wallet,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let cmd = LayerCheckoutCommand {
        actor,
        wallet,
        layer,
        offer,
        version,
        expected_price: price,
        key,
    };
    match PgLayers::new(state.accounts.pool().clone()).buy(&cmd).await {
        Ok(receipt) => private_response(
            Json(LayerPurchaseReceipt {
                term_id: receipt.term_id.to_string(),
                balance_minor: receipt.balance_after_minor,
                replayed: receipt.replayed,
            }),
            &[],
        ),
        Err(error) => deny(layer_status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn d_05_layer_checkout_rejects_bad_ids_and_maps_failures() {
        for bad in ["", "0", "-1", "12a", "+1", "18446744073709551615"] {
            assert_eq!(positive(bad), Err(StatusCode::UNPROCESSABLE_ENTITY));
        }
        assert_eq!(positive("12").unwrap().get(), 12);
        assert_eq!(
            layer_status(LayerError::ReplayConflict),
            StatusCode::CONFLICT
        );
        assert_eq!(
            layer_status(LayerError::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
