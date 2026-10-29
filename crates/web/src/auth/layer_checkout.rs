//! S031: explicit Layer wallet purchase, opt-in and never an external charge.
//! No automatic renewal, creator posting, refund or withdrawable balances.
use super::{deny, layer_books, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::{
        layers::{LayerCheckoutCommand, LayerOfferQuote, LayerTermKind},
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
    services::layers::LayerService,
};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PurchaseForm {
    expected_offer_id: i64,
    expected_version_id: i64,
    expected_price_minor: i64,
    command_key: String,
    csrf_token: Option<String>,
}

fn enabled(state: &AuthState) -> bool {
    state.paid_checkout_enabled
        && std::env::var("TOR_LAYER_CHECKOUT_ENABLED").as_deref() == Ok("1")
}

pub(super) fn routes() -> Router<AuthState> {
    Router::new()
        .route("/layers/{layer_id}", get(page))
        .route("/layers/{layer_id}/buy", post(buy))
}

fn quote_html(quote: &LayerOfferQuote, csrf: &str, key: &str) -> Html<String> {
    let mut books = String::from("<ul class=\"author-book-list\">");
    for book in &quote.books {
        write!(
            books,
            "<li>{}</li>",
            pages::escape_html(&book.title)
        )
        .expect("write String");
    }
    books.push_str("</ul>");
    let duration = match quote.term_kind {
        LayerTermKind::Prepaid30Days => "30 дней, только ручное продление",
        LayerTermKind::Lifetime => "Бессрочно, включая будущие книги Layer",
    };
    let rubles = quote.price_minor / 100;
    let kopecks = quote.price_minor % 100;
    let title = pages::escape_html(&quote.title);
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card">
        <p class="eyebrow">Подписка Layer</p><h1>{title}</h1>
        <p>Стоимость: <strong>{rubles},{kopecks:02} ₽</strong>; {duration}.</p>
        <p>Покупка оплачивается только из внутреннего RUB-кошелька.
        Автосписаний и внешнего платежа нет. Покупка закрепляет показанную
        версию условий и состава. Книги могут добавляться позже.</p>
        <h2>Книги в текущем составе ({})</h2>{books}
        <form method="post" action="/layers/{}/buy">
        <input type="hidden" name="expected_offer_id" value="{}">
        <input type="hidden" name="expected_version_id" value="{}">
        <input type="hidden" name="expected_price_minor" value="{}">
        <input type="hidden" name="command_key" value="{}">
        <input type="hidden" name="csrf_token" value="{}">
        <button class="button" type="submit">Оплатить из кошелька</button>
        </form>
        <p>Повторная самостоятельная оплата создаёт новый срок.
        Повтор этой же формы не создаёт второго списания.</p>
        </div></div></section>"#,
        quote.books.len(),
        quote.layer_id,
        quote.offer_id,
        quote.version_id,
        quote.price_minor,
        pages::escape_html(key),
        pages::escape_html(csrf),
    );
    pages::layout("Купить Layer", "Проверенная цена и состав Layer.", &content)
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !enabled(&state) {
        return deny(StatusCode::NOT_FOUND);
    }
    let layer = match PositiveId::new(raw_layer) {
        Ok(layer) => layer,
        Err(_) => return deny(StatusCode::NOT_FOUND),
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    let quote = match service.quote(layer).await {
        Ok(quote) => quote,
        Err(error) => return deny(layer_books::status(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    private_response(quote_html(&quote, &csrf, &key), &cookies)
}

pub(super) async fn buy(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PurchaseForm>,
) -> Response {
    if !enabled(&state) {
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
    let (Ok(actor), Ok(layer), Ok(offer), Ok(version), Ok(key)) = (
        PositiveId::new(actor),
        PositiveId::new(raw_layer),
        PositiveId::new(form.expected_offer_id),
        PositiveId::new(form.expected_version_id),
        IdempotencyKey::parse(&form.command_key),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let price = match MoneyMinor::new(form.expected_price_minor) {
        Ok(price) if price.get() > 0 => price,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let wallet = match WalletId::new(actor.get()) {
        Ok(wallet) => wallet,
        Err(_) => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let command = LayerCheckoutCommand {
        actor,
        wallet,
        layer,
        offer,
        version,
        expected_price: price,
        key,
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    match service.purchase(&command).await {
        Ok(receipt) => private_response(
            Html(format!(
                "<main><h1>Оплата Layer подтверждена</h1>\
                 <p>Номер оплаченного срока: {}</p>\
                 <p>Баланс кошелька после оплаты: {} коп.</p>\
                 <p>{}</p>\
                 <p><a href=\"/me\">Личный кабинет</a></p></main>",
                receipt.term_id,
                receipt.balance_after_minor,
                if receipt.replayed {
                    "Повторное списание не выполнялось."
                } else {
                    "Создан отдельный оплаченный срок."
                },
            )),
            &[],
        ),
        Err(error) => deny(layer_books::status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_commerce::domain::layers::LayerBookCandidate;

    #[test]
    fn d_05_layer_quote_escapes_names_and_shows_exact_terms() {
        let q = LayerOfferQuote {
            layer_id: 17,
            title: "<script>alert(1)</script>".into(),
            offer_id: 18,
            version_id: 19,
            term_kind: LayerTermKind::Prepaid30Days,
            price_minor: 251,
            books: vec![LayerBookCandidate {
                book_id: 42,
                title: "<img onerror=1>".into(),
            }],
        };
        let html = quote_html(&q, "csrf", "key").0;
        assert!(html.contains("2,51 ₽"));
        assert!(html.contains("30 дней, только ручное продление"));
        assert!(html.contains("name=\"expected_offer_id\" value=\"18\""));
        assert!(html.contains("name=\"expected_version_id\" value=\"19\""));
        assert!(html.contains("name=\"expected_price_minor\" value=\"251\""));
        assert!(html.contains("name=\"command_key\""));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img onerror"));
    }

    #[test]
    fn d_05_lifetime_quote_does_not_claim_automatic_payments() {
        let q = LayerOfferQuote {
            layer_id: 17,
            title: "Layer".into(),
            offer_id: 18,
            version_id: 19,
            term_kind: LayerTermKind::Lifetime,
            price_minor: 500,
            books: vec![LayerBookCandidate {
                book_id: 42,
                title: "Book".into(),
            }],
        };
        let html = quote_html(&q, "csrf", "key").0;
        assert!(html.contains("Бессрочно, включая будущие книги Layer"));
        assert!(html.contains("Автосписаний и внешнего платежа нет"));
    }
}
