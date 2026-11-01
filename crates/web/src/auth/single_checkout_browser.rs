//! S005/S034: a deliberate one-chapter purchase using the existing D ledger.
//! Not an external payment provider. Never exposes signed S3 credentials.
use super::{checkout::checkout_status, deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::money::{
        CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::PublishedChapterOffer,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PurchaseForm {
    expected_version_id: String,
    expected_price_minor: i64,
    command_key: String,
    csrf_token: String,
}

fn noindex(mut response: Response) -> Response {
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

fn layout(title: &str, content: &str) -> Html<String> {
    pages::layout(title, "Покупка опубликованной платной главы.", &format!(
        "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card reader-access-page\">{content}</div></div></section>"
    ))
}

fn offer_form(offer: &PublishedChapterOffer, csrf: &str, key: &str) -> Html<String> {
    let price = format!("{},{:02}", offer.price_minor / 100, offer.price_minor % 100);
    let text = format!(
        "<h1>Подтвердить покупку</h1>
         <p><strong>{book}</strong> · Глава {chapter_no}. {chapter}</p>
         <p>Из внутреннего кошелька будет списано <strong>{price} ₽</strong>.</p>
         <p>Покупается именно указанная редакция главы. Новые редакции могут продаваться отдельно.</p>
         <form method=\"post\" action=\"/me/chapters/{id}/buy\">
         <input type=\"hidden\" name=\"expected_version_id\" value=\"{version}\">
         <input type=\"hidden\" name=\"expected_price_minor\" value=\"{minor}\">
         <input type=\"hidden\" name=\"command_key\" value=\"{key}\">
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">
         <button class=\"button\" type=\"submit\">Подтвердить покупку за {price} ₽</button>
         </form>
         <p>При недостаточном балансе или изменении цены деньги не списываются. Внешних платежей здесь нет.</p>
         <p><a href=\"/books/{book_id}#chapters\">К оглавлению</a> · <a href=\"/me/purchases\">Мои покупки</a></p>",
        book = pages::escape_html(&offer.book_title),
        chapter_no = offer.chapter_no,
        chapter = pages::escape_html(&offer.chapter_title),
        id = offer.chapter_id,
        version = offer.version_id,
        minor = offer.price_minor,
        key = pages::escape_html(key),
        csrf = pages::escape_html(csrf),
        book_id = offer.book_id,
    );
    layout("Подтверждение покупки", &text)
}

fn failure(chapter_id: i64, status: StatusCode, message: &str) -> Response {
    noindex(private_response(
        (status, layout("Покупка не выполнена", &format!(
            "<h1>Покупка не выполнена</h1><p role=\"alert\">{}</p>
             <p><a href=\"/me/chapters/{chapter_id}/buy\">Открыть предложение заново</a></p>
             <p><a href=\"/me/purchases\">Мои покупки</a></p>",
            pages::escape_html(message),
        ))),
        &[],
    ))
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !state.paid_checkout_enabled || !state.paid_reader_available {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return noindex(private_response(Redirect::to("/login"), &[])),
        Err(error) => return deny(error),
    };
    match state.checkout.purchased_chapter(actor, chapter_id).await {
        Ok(true) => return noindex(private_response(Redirect::to("/me/purchases"), &[])),
        Ok(false) => {}
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let offer = match state.checkout.published_chapter_offer(actor, chapter_id).await {
        Ok(Some(offer)) => offer,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    noindex(private_response(
        offer_form(&offer, &csrf, &generate_session_token()),
        &cookies,
    ))
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PurchaseForm>,
) -> Response {
    if !state.paid_checkout_enabled || !state.paid_reader_available {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Ok(chapter), Ok(version), Ok(actor_id), Ok(wallet), Ok(price), Ok(key)) = (
        PositiveId::new(chapter_id),
        form.expected_version_id.parse::<i64>().ok().and_then(|v| PositiveId::new(v).ok()).ok_or(()),
        PositiveId::new(actor),
        WalletId::new(actor),
        MoneyMinor::new(form.expected_price_minor),
        IdempotencyKey::parse(&form.command_key),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    if price.get() <= 0 || form.expected_version_id.len() > 19
        || !form.expected_version_id.bytes().all(|b| b.is_ascii_digit())
    {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let ready = match state.checkout.browser_offer_ready_or_replay(
        actor,
        chapter_id,
        version.get(),
        price.get(),
        key.as_str(),
    ).await {
        Ok(ready) => ready,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !ready {
        return failure(chapter_id, StatusCode::CONFLICT,
            "Цена или редакция изменилась либо защищённый файл не готов. Обновите предложение.");
    }
    let command = match PurchaseCommand::new(
        actor_id,
        wallet,
        ChapterVersionRef { chapter_id: chapter, version_id: version },
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
                CheckoutError::InsufficientBalance => "Недостаточно средств. Списания не было.",
                CheckoutError::StaleOffer => "Цена или редакция изменилась. Обновите предложение.",
                CheckoutError::AlreadyOwned => "Глава уже куплена. Посмотрите свои покупки.",
                CheckoutError::DuplicateKeyDifferentRequest => "Подтверждение относится к другой покупке.",
                CheckoutError::Inaccessible => "Сейчас глава недоступна для покупки.",
                CheckoutError::Unavailable => "Покупка временно недоступна. Повторите попытку.",
            };
            failure(chapter_id, checkout_status(error), message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_chapter_form_escapes_metadata_and_keeps_actor_out_of_html() {
        let offer = PublishedChapterOffer {
            book_id: 20,
            book_title: "<script>book</script>".into(),
            chapter_id: 103,
            chapter_no: 3,
            chapter_title: "<title>".into(),
            version_id: 111,
            price_minor: 399,
        };
        let html = offer_form(&offer, "csrf", "key").0;
        assert!(html.contains("&lt;script&gt;book&lt;/script&gt;"));
        assert!(html.contains("&lt;title&gt;"));
        assert!(!html.contains("<script>book</script>"));
        assert!(html.contains("name=\"expected_version_id\" value=\"111\""));
        assert!(html.contains("3,99 ₽"));
        assert!(!html.contains("name=\"wallet_id\""));
        assert!(!html.contains("name=\"actor\""));
    }
}
