//! S034: explicit native purchase confirmation using the existing D transaction.
use super::{
    checkout::{checkout_status, positive},
    deny, session_user, AuthState,
};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::domain::money::{
    CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

pub(super) fn router() -> Router<AuthState> {
    Router::new().route(
        "/me/chapters/{id}/buy",
        get(browser_offer)
            .post(browser_buy)
            .layer(DefaultBodyLimit::max(2048)),
    )
}

/// A real HTML form: no untrusted actor/wallet, no client-side JSON scripting.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BrowserBuyChapter {
    expected_version_id: String,
    expected_price_minor: i64,
    command_key: String,
    csrf_token: String,
}

/// S034: browser checkout never mounts without both the private paid reader
/// and the explicit wallet-checkout switch. No remote payment gateway here.
fn browser_enabled(state: &AuthState) -> bool {
    state.paid_checkout_enabled && state.paid_reader_available
}

fn noindex(mut response: Response) -> Response {
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn browser_error(chapter_id: i64, status: StatusCode, message: &str) -> Response {
    noindex(private_response(
        (status, pages::chapter_purchase_problem(chapter_id, message)),
        &[],
    ))
}

/// GET shows the exact B-published version and price, or redirects an
/// existing buyer to their authorized purchase library without charging.
pub(super) async fn browser_offer(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !browser_enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.checkout.purchased_chapter(actor, chapter_id).await {
        Ok(true) => return noindex(private_response(Redirect::to("/me/purchases"), &[])),
        Ok(false) => {}
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let offer = match state.checkout.published_chapter_offer(chapter_id).await {
        Ok(Some(offer)) => offer,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // A fresh random purchase intent and a matching CSRF cookie are issued
    // together. Reloading the form must never reuse another buyer's key.
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    noindex(private_response(
        pages::chapter_purchase_form(&offer, &csrf, &key),
        &cookies,
    ))
}

/// POST repeats the complete D command under PG locks and redirects only to
/// the paid reader for the exact purchase receipt returned by that commit.
pub(super) async fn browser_buy(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<BrowserBuyChapter>,
) -> Response {
    if !browser_enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Ok(chapter), Ok(version), Ok(wallet), Ok(actor_id), Ok(price), Ok(key)) = (
        PositiveId::new(chapter_id),
        positive(&form.expected_version_id),
        WalletId::new(actor),
        PositiveId::new(actor),
        MoneyMinor::new(form.expected_price_minor),
        IdempotencyKey::parse(&form.command_key),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    if price.get() <= 0 {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let allowed = match state
        .checkout
        .browser_offer_ready_or_replay(actor, chapter_id, version.get(), price.get(), key.as_str())
        .await
    {
        Ok(allowed) => allowed,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !allowed {
        return browser_error(
            chapter_id,
            StatusCode::CONFLICT,
            "Цена или редакция изменились, либо защищённый файл ещё не готов. Откройте предложение заново.",
        );
    }
    let command = match PurchaseCommand::new(
        actor_id,
        wallet,
        ChapterVersionRef {
            chapter_id: chapter,
            version_id: version,
        },
        price,
        key,
    ) {
        Ok(command) => command,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match state.checkout.checkout_once(&command).await {
        Ok(receipt) => noindex(private_response(
            Redirect::to(&format!(
                "/read/paid/{}?purchase_id={}",
                receipt.pinned.chapter_id.get(),
                receipt.purchase_id.get()
            )),
            &[],
        )),
        Err(error) => {
            let message = match error {
                CheckoutError::InsufficientBalance => {
                    "Недостаточно средств во внутреннем кошельке. Деньги не списаны."
                }
                CheckoutError::StaleOffer => {
                    "Цена или редакция изменились. Откройте предложение заново."
                }
                CheckoutError::AlreadyOwned => {
                    "Глава уже была куплена. Проверьте свои покупки и доступ к чтению."
                }
                CheckoutError::DuplicateKeyDifferentRequest => {
                    "Это подтверждение относится к другой покупке. Откройте предложение заново."
                }
                CheckoutError::Inaccessible => "Сейчас эта глава недоступна для покупки.",
                CheckoutError::Unavailable => "Покупка временно недоступна. Повторите попытку.",
            };
            browser_error(chapter_id, checkout_status(error), message)
        }
    }
}
