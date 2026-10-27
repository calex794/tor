//! D-04: controlled internal-wallet checkout API. No external payment flow.
//! The route is disabled unless TOR_PAID_CHECKOUT_ENABLED=1 was set at startup.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use tor_commerce::domain::money::{
    CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuyChapter {
    chapter_id: String,
    expected_version_id: String,
    expected_price_minor: i64,
}

#[derive(Serialize)]
struct BuyReceipt {
    purchase_id: String,
    chapter_id: String,
    version_id: String,
    balance_minor: i64,
    replayed: bool,
}

fn positive(raw: &str) -> Result<PositiveId, StatusCode> {
    if raw.is_empty() || raw.len() > 19 || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let id = raw
        .parse::<i64>()
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    PositiveId::new(id).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)
}

fn checkout_status(error: CheckoutError) -> StatusCode {
    match error {
        CheckoutError::InsufficientBalance
        | CheckoutError::StaleOffer
        | CheckoutError::AlreadyOwned
        | CheckoutError::DuplicateKeyDifferentRequest => StatusCode::CONFLICT,
        CheckoutError::Inaccessible => StatusCode::NOT_FOUND,
        CheckoutError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) async fn buy(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(input): Json<BuyChapter>,
) -> Response {
    if !state.paid_checkout_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !state.cookies.same_origin(&headers)
        || !headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|token| state.cookies.valid_csrf(&headers, token))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    // The key belongs in the HTTP header, never in the JSON body. Actor and
    // wallet IDs are server-derived exclusively from the revocable session.
    let key = match headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| IdempotencyKey::parse(raw).ok())
    {
        Some(key) => key,
        None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let actor = match PositiveId::new(actor) {
        Ok(actor) => actor,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let (chapter_id, version_id) = match (
        positive(&input.chapter_id),
        positive(&input.expected_version_id),
    ) {
        (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
        (Err(status), _) | (_, Err(status)) => return deny(status),
    };
    let price = match MoneyMinor::new(input.expected_price_minor) {
        Ok(price) if price.get() > 0 => price,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let wallet = match WalletId::new(actor.get()) {
        Ok(wallet) => wallet,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let command = match PurchaseCommand::new(
        actor,
        wallet,
        ChapterVersionRef {
            chapter_id,
            version_id,
        },
        price,
        key,
    ) {
        Ok(command) => command,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match state.checkout.checkout_once(&command).await {
        Ok(receipt) => private_response(
            Json(BuyReceipt {
                purchase_id: receipt.purchase_id.get().to_string(),
                chapter_id: receipt.pinned.chapter_id.wire(),
                version_id: receipt.pinned.version_id.wire(),
                balance_minor: receipt.balance_after.get(),
                replayed: receipt.replayed,
            }),
            &[],
        ),
        Err(error) => deny(checkout_status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_04_rejects_invalid_identifiers_and_money_errors() {
        for bad in ["", "0", "-1", "12a", "+1", "18446744073709551615"] {
            assert_eq!(positive(bad), Err(StatusCode::UNPROCESSABLE_ENTITY));
        }
        assert_eq!(positive("123").unwrap().get(), 123);
        assert_eq!(
            checkout_status(CheckoutError::AlreadyOwned),
            StatusCode::CONFLICT
        );
        assert_eq!(
            checkout_status(CheckoutError::Unavailable),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
