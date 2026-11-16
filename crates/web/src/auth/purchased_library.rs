//! S034: owned purchase receipts are inspectable without a configured S3 reader.
//! Only D-authorized metadata is returned. No paid text, S3 grants or keys.
use super::{checkout, deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_commerce::repositories::entitlements::PgDirectEntitlements;
use tor_contracts::paid_delivery::PositiveId;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PurchasesQuery {
    after: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/me/purchases", get(page))
}

async fn page(
    State(state): State<AuthState>,
    Query(query): Query<PurchasesQuery>,
    headers: HeaderMap,
) -> Response {
    if !state.paid_checkout_enabled || state.paid_reader_available {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let actor = match PositiveId::new(actor) {
        Ok(actor) => actor,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let after = match query.after {
        Some(raw) => match checkout::positive(&raw) {
            Ok(after) => Some(after),
            Err(status) => return deny(status),
        },
        None => None,
    };
    let owned = match PgDirectEntitlements::new(state.accounts.pool().clone())
        .readable_purchases(actor, after)
        .await
    {
        Ok(owned) => owned,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut response = private_response(
        pages::purchased_chapters_page_with_reader(&owned, false),
        &[],
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
        .headers_mut()
        .insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}
