//! D/S035: batch counterpart of D-04 single-chapter HTTP checkout.
//! The actor, wallet, CSRF and idempotency key are never trusted from JSON.
use super::{checkout_status, positive};
use crate::auth::{deny, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use tor_commerce::domain::{
    batch_checkout::{BatchChapterOffer, BatchCheckoutError, BatchPurchaseCommand, MAX_BATCH_CHAPTERS},
    money::{IdempotencyKey, MoneyMinor, WalletId},
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    chapter_id: String,
    expected_version_id: String,
    expected_price_minor: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BatchSelection {
    chapters: Vec<Selection>,
}

#[derive(Serialize)]
struct ItemReceipt {
    purchase_id: String,
    chapter_id: String,
    version_id: String,
    balance_minor: i64,
    replayed: bool,
}

#[derive(Serialize)]
struct BatchReceipt {
    purchases: Vec<ItemReceipt>,
    total_minor: i64,
    balance_minor: i64,
    replayed: bool,
}

/// Invoked only after the shared checkout handler has authenticated the
/// session and verified same-origin CSRF at the route boundary.
pub(super) async fn buy_for_actor(
    state: &AuthState,
    actor: i64,
    headers: &HeaderMap,
    input: BatchSelection,
) -> Response {
    if !(2..=MAX_BATCH_CHAPTERS).contains(&input.chapters.len()) {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let key = match headers
        .get("idempotency-key")
        .and_then(|header| header.to_str().ok())
        .and_then(|value| IdempotencyKey::parse(value).ok())
    {
        Some(key) => key,
        None => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let actor = match PositiveId::new(actor) {
        Ok(actor) => actor,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let wallet = match WalletId::new(actor.get()) {
        Ok(wallet) => wallet,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let mut offers = Vec::with_capacity(input.chapters.len());
    for selected in input.chapters {
        let (chapter_id, version_id) = match (
            positive(&selected.chapter_id),
            positive(&selected.expected_version_id),
        ) {
            (Ok(chapter), Ok(version)) => (chapter, version),
            _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        };
        let expected_price = match MoneyMinor::new(selected.expected_price_minor) {
            Ok(price) if price.get() > 0 => price,
            _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
        };
        offers.push(BatchChapterOffer {
            edition: ChapterVersionRef {
                chapter_id,
                version_id,
            },
            expected_price,
        });
    }
    let command = match BatchPurchaseCommand::new(actor, wallet, offers, key) {
        Ok(command) => command,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match state.checkout.checkout_batch_once(&command).await {
        Ok(receipt) => private_response(
            Json(BatchReceipt {
                total_minor: receipt.total.get(),
                balance_minor: receipt.balance_after.get(),
                replayed: receipt.replayed,
                purchases: receipt
                    .purchases
                    .into_iter()
                    .map(|item| ItemReceipt {
                        purchase_id: item.purchase_id.get().to_string(),
                        chapter_id: item.pinned.chapter_id.wire(),
                        version_id: item.pinned.version_id.wire(),
                        balance_minor: item.balance_after.get(),
                        replayed: item.replayed,
                    })
                    .collect(),
            }),
            &[],
        ),
        Err(BatchCheckoutError::InvalidBatch | BatchCheckoutError::MixedBooks) => {
            deny(StatusCode::UNPROCESSABLE_ENTITY)
        }
        Err(BatchCheckoutError::Checkout(error)) => deny(checkout_status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_does_not_accept_client_provided_wallet_or_malformed_offer() {
        assert!(serde_json::from_str::<BatchSelection>(
            r#"{"chapters":[{"chapter_id":"1","expected_version_id":"2","expected_price_minor":100}],"user_id":"99"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<BatchSelection>(
            r#"{"chapters":[{"chapter_id":1,"expected_version_id":"2","expected_price_minor":100}]}"#
        )
        .is_err());
    }
}
