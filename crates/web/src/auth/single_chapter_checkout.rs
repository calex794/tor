//! D-07: single paid-chapter checkout with a subscriber-specific exact quote.
use super::{checkout::checkout_status, deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::money::{
        CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::SingleChapterQuote,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FormBuy {
    csrf_token: Option<String>,
    expected_version_id: i64,
    expected_price_minor: i64,
    expected_base_price_minor: i64,
    expected_campaign_id: i64,
    expected_campaign_revision: i32,
    expected_rule_version: i16,
    expected_follow_period_id: i64,
    expected_discount_percent: i16,
    expected_min_follow_days: i16,
    command_key: String,
}

fn money(amount: i64) -> String {
    format!("{},{:02} ₽", amount / 100, amount % 100)
}

fn shell(book_id: i64, body: &str) -> Html<String> {
    pages::layout(
        "Покупка главы",
        "Списание из внутреннего кошелька только после подтверждения.",
        &format!(
            "<section class=\"section auth-section\"><div class=\"container\">\
             <div class=\"auth-card\"><h1>Покупка главы</h1>{body}\
             <p><a href=\"/books/{book_id}#chapters\">Вернуться к книге</a></p>\
             </div></div></section>",
        ),
    )
}

fn confirm(quote: &SingleChapterQuote, csrf: &str) -> Html<String> {
    let details = if let Some(percent) = quote.discount_percent {
        format!(
            "<p>Обычная цена: {}. Скидка {}% для подписчика{}.</p>",
            money(quote.base_price_minor),
            percent,
            if quote.requires_follow_days == Some(90) {
                " со стажем от 90 дней"
            } else {
                ""
            },
        )
    } else {
        "<p>Скидка для этого читателя сейчас не действует.</p>".to_owned()
    };
    shell(
        quote.book_id,
        &format!(
            "<p>Книга: {}</p><p>Глава {}. {}</p>{details}\
             <p><strong>К списанию: {}</strong></p>\
             <form method=\"post\" action=\"/me/books/{}/buy-chapter/{}\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_version_id\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_price_minor\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_base_price_minor\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_campaign_id\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_campaign_revision\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_rule_version\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_follow_period_id\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_discount_percent\" value=\"{}\">\
             <input type=\"hidden\" name=\"expected_min_follow_days\" value=\"{}\">\
             <input type=\"hidden\" name=\"command_key\" value=\"{}\">\
             <button type=\"submit\" class=\"ui-button ui-button--primary\">Купить главу</button>\
             </form>\
             <p>При изменении исходной или итоговой цены, акции, периода подписки или редакции покупка будет отклонена без списания.</p>",
            pages::escape_html(&quote.book_title),
            quote.chapter_no,
            pages::escape_html(&quote.chapter_title),
            money(quote.final_price_minor),
            quote.book_id,
            quote.chapter_id,
            pages::escape_html(csrf),
            quote.version_id,
            quote.final_price_minor,
            quote.base_price_minor,
            quote.campaign_id.unwrap_or(0),
            quote.campaign_revision.unwrap_or(0),
            quote.rule_version.unwrap_or(0),
            quote.follow_period_id.unwrap_or(0),
            quote.discount_percent.unwrap_or(0),
            quote.requires_follow_days.unwrap_or(0),
            generate_session_token(),
        ),
    )
}

fn failed(book_id: i64, error: CheckoutError) -> Response {
    let (status, message) = match error {
        CheckoutError::Inaccessible => (StatusCode::NOT_FOUND, "Глава недоступна."),
        CheckoutError::StaleOffer => (
            StatusCode::CONFLICT,
            "Цена, акция, стаж подписки или редакция изменилась. Обновите предложение.",
        ),
        CheckoutError::AlreadyOwned => (
            StatusCode::CONFLICT,
            "Глава уже куплена. Повторного списания нет.",
        ),
        CheckoutError::InsufficientBalance => (
            StatusCode::CONFLICT,
            "Недостаточно средств во внутреннем кошельке.",
        ),
        _ => (
            checkout_status(error),
            "Не удалось подтвердить покупку. Списание не выполнено.",
        ),
    };
    private_response(
        (
            status,
            shell(
                book_id,
                &format!(
        "<p role=\"alert\">{message}</p><p><a href=\"/me/library\">Моя библиотека</a></p>",
    ),
            ),
        ),
        &[],
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Response {
    if !state.paid_checkout_enabled || book_id <= 0 || chapter_id <= 0 {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let quote = match state
        .checkout
        .quote_single(actor, book_id, chapter_id)
        .await
    {
        Ok(offer) => offer,
        Err(error) => return failed(book_id, error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(confirm(&quote, &csrf), &cookies)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<FormBuy>,
) -> Response {
    if !state.paid_checkout_enabled || book_id <= 0 || chapter_id <= 0 {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    // The authenticated chapter must actually belong to this public book.
    // Rechecking the quote here is a path guard; D rechecks price, E period,
    // campaign and pinned version under locks in the purchase transaction.
    match state
        .checkout
        .quote_single(actor, book_id, chapter_id)
        .await
    {
        Ok(_) | Err(CheckoutError::AlreadyOwned) => {}
        Err(error) => return failed(book_id, error),
    }
    let command = (|| {
        let command = PurchaseCommand::new(
            PositiveId::new(actor).ok()?,
            WalletId::new(actor).ok()?,
            ChapterVersionRef {
                chapter_id: PositiveId::new(chapter_id).ok()?,
                version_id: PositiveId::new(form.expected_version_id).ok()?,
            },
            MoneyMinor::new(form.expected_price_minor).ok()?,
            IdempotencyKey::parse(&form.command_key).ok()?,
        )
        .ok()?;
        let base = MoneyMinor::new(form.expected_base_price_minor).ok()?;
        if form.expected_campaign_id == 0
            && form.expected_campaign_revision == 0
            && form.expected_rule_version == 0
            && form.expected_follow_period_id == 0
            && form.expected_discount_percent == 0
            && form.expected_min_follow_days == 0
        {
            (base == MoneyMinor::new(form.expected_price_minor).ok()?).then_some(command)
        } else {
            command
                .with_discount_offer(
                    base,
                    form.expected_campaign_id,
                    form.expected_campaign_revision,
                    form.expected_rule_version,
                    form.expected_follow_period_id,
                    form.expected_discount_percent,
                    form.expected_min_follow_days,
                )
                .ok()
        }
    })();
    let Some(command) = command else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    match state.checkout.checkout_once(&command).await {
        Ok(receipt) => {
            let reader_link = if state.paid_reader_available {
                format!(
                    "<p><a href=\"/read/paid/{chapter_id}?purchase_id={}\">Читать купленную главу</a></p>",
                    receipt.purchase_id.get(),
                )
            } else {
                "<p>Защищённая читалка доступна после включения доставки платных глав.</p>"
                    .to_owned()
            };
            private_response(
                shell(
                    book_id,
                    &format!(
                        "<p role=\"status\">Глава куплена. Списано {}. Остаток {}.</p>\
                 <p>{}</p>{reader_link}<p><a href=\"/me/purchases\">Мои покупки</a></p>",
                        money(receipt.amount.get()),
                        money(receipt.balance_after.get()),
                        if receipt.replayed {
                            "Повторное подтверждение: новых списаний нет."
                        } else {
                            ""
                        },
                    ),
                ),
                &[],
            )
        }
        Err(error) => failed(book_id, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quote_escapes_untrusted_titles_and_shows_exact_minor() {
        let quote = SingleChapterQuote {
            book_id: 1,
            chapter_id: 2,
            chapter_no: 3,
            chapter_title: "<script>bad</script>".to_owned(),
            book_title: "Книга".to_owned(),
            version_id: 4,
            base_price_minor: 100,
            final_price_minor: 80,
            campaign_id: Some(10),
            campaign_revision: Some(1),
            rule_version: Some(1),
            follow_period_id: Some(20),
            discount_percent: Some(20),
            requires_follow_days: Some(90),
        };
        let html = confirm(&quote, "csrf").0;
        assert!(html.contains("К списанию: 0,80 ₽"));
        assert!(html.contains("Обычная цена: 1,00 ₽"));
        assert!(html.contains("со стажем от 90 дней"));
        assert!(!html.contains("<script>bad</script>"));
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(html.contains("expected_version_id"));
        assert!(html.contains("expected_base_price_minor"));
        assert!(html.contains("expected_campaign_revision"));
        assert!(html.contains("expected_rule_version"));
        assert!(html.contains("expected_follow_period_id"));
    }
}
