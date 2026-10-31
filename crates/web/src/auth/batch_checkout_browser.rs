//! S035: two-step native browser checkout. No scripts or client-supplied wallet.
use super::{checkout::checkout_status, deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use std::{collections::HashMap, fmt::Write};
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::{
        batch_checkout::{
            BatchChapterOffer, BatchCheckoutError, BatchPurchaseCommand, MAX_BATCH_CHAPTERS,
        },
        money::{CheckoutError, IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::checkout::SelectableBatchOffer,
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

fn money(amount: i64) -> String {
    format!("{},{:02} ₽", amount / 100, amount % 100)
}

fn shell(title: &str, book_id: i64, contents: &str) -> axum::response::Html<String> {
    pages::layout(
        title,
        "Покупка нескольких глав книги из внутреннего кошелька.",
        &format!(
            "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card batch-purchase\">
             <p class=\"eyebrow\">Главы книги · внутренний кошелёк</p><h1>{}</h1>{contents}
             <p><a class=\"text-link\" href=\"/books/{book_id}#chapters\">Вернуться к оглавлению</a></p>
             </div></div></section>",
            pages::escape_html(title),
        ),
    )
}

fn select_page(
    book_id: i64,
    title: &str,
    csrf: &str,
    offers: &[SelectableBatchOffer],
) -> axum::response::Html<String> {
    let mut rows = String::new();
    for item in offers.iter().take(100) {
        write!(
            rows,
            "<label class=\"batch-purchase__row\"><input type=\"checkbox\" name=\"chapter_{}\" value=\"1\">
             <span>Глава {}. {}</span><strong>{}</strong></label>",
            item.chapter_id,
            item.chapter_no,
            pages::escape_html(&item.title),
            money(item.price_minor)
        )
        .expect("rendering HTML into String");
    }
    let action = if offers.len() < 2 {
        "<p role=\"status\">Сейчас нет двух доступных для покупки глав. Уже купленные главы повторно не предлагаются.</p>".to_owned()
    } else {
        format!(
            "<form method=\"post\" action=\"/me/books/{book_id}/buy-chapters\" class=\"batch-purchase__form\">
             <fieldset><legend>Выберите от 2 до 20 глав одной книги</legend>{rows}</fieldset>
             <input type=\"hidden\" name=\"step\" value=\"select\">
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
             <p>Стоимость и окончательный список будут показаны перед списанием. Скидки подписчикам действуют только при покупке одной главы.</p>
             <button class=\"ui-button ui-button--primary\" type=\"submit\">Проверить выбор и цену</button></form>",
            pages::escape_html(csrf)
        )
    };
    let overflow = if offers.len() > 100 {
        "<p>Показаны первые 100 доступных глав. Остальные появятся после расширения выбора.</p>"
    } else {
        ""
    };
    shell(
        "Купить несколько глав",
        book_id,
        &format!("<h2>{}</h2>{action}{overflow}", pages::escape_html(title)),
    )
}

fn selected(form: &HashMap<String, String>) -> Option<Vec<i64>> {
    if form.len() > MAX_BATCH_CHAPTERS + 2 {
        return None;
    }
    let mut result = Vec::new();
    for (key, value) in form {
        if key == "step" || key == "csrf_token" {
            continue;
        }
        let id = key.strip_prefix("chapter_")?;
        if value != "1" || id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        result.push(id.parse::<i64>().ok().filter(|x| *x > 0)?);
    }
    result.sort_unstable();
    if !(2..=MAX_BATCH_CHAPTERS).contains(&result.len())
        || result.windows(2).any(|pair| pair[0] == pair[1])
    {
        return None;
    }
    Some(result)
}

fn parse_numbers(value: Option<&String>) -> Option<Vec<i64>> {
    let raw = value?;
    if raw.len() > 420 || raw.is_empty() {
        return None;
    }
    raw.split(',')
        .map(|s| {
            if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            s.parse::<i64>().ok().filter(|v| *v > 0)
        })
        .collect()
}

fn confirm_page(
    book_id: i64,
    title: &str,
    csrf: &str,
    offers: &[SelectableBatchOffer],
) -> Option<axum::response::Html<String>> {
    let mut total = 0_i64;
    let mut rows = String::new();
    for offer in offers {
        total = total.checked_add(offer.price_minor)?;
        write!(
            rows,
            "<li>Глава {}. {} — {}</li>",
            offer.chapter_no,
            pages::escape_html(&offer.title),
            money(offer.price_minor)
        )
        .expect("rendering HTML into String");
    }
    let join = |field: fn(&SelectableBatchOffer) -> i64| {
        offers
            .iter()
            .map(|offer| field(offer).to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let content = format!(
        "<h2>{}</h2><p>Вы выбрали {} глав. Списание будет выполнено только целиком.</p>
         <ol>{rows}</ol><p class=\"batch-purchase__total\">Итого: {}</p>
         <form method=\"post\" action=\"/me/books/{book_id}/buy-chapters\">
         <input type=\"hidden\" name=\"step\" value=\"commit\">
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
         <input type=\"hidden\" name=\"command_key\" value=\"{}\">
         <input type=\"hidden\" name=\"chapter_ids\" value=\"{}\">
         <input type=\"hidden\" name=\"version_ids\" value=\"{}\">
         <input type=\"hidden\" name=\"prices\" value=\"{}\">
         <button class=\"ui-button ui-button--primary\" type=\"submit\">Подтвердить покупку на {}</button>
         </form><p>Если цена или версия изменится, операция будет отклонена без частичного списания.</p>",
        pages::escape_html(title),
        offers.len(),
        money(total),
        pages::escape_html(csrf),
        generate_session_token(),
        join(|item| item.chapter_id),
        join(|item| item.version_id),
        join(|item| item.price_minor),
        money(total)
    );
    Some(shell("Подтверждение покупки", book_id, &content))
}

fn command(actor: i64, fields: &HashMap<String, String>) -> Option<BatchPurchaseCommand> {
    if fields.len() != 6 {
        return None;
    }
    let chapters = parse_numbers(fields.get("chapter_ids"))?;
    let versions = parse_numbers(fields.get("version_ids"))?;
    let prices = parse_numbers(fields.get("prices"))?;
    if !(2..=MAX_BATCH_CHAPTERS).contains(&chapters.len())
        || versions.len() != chapters.len()
        || prices.len() != chapters.len()
        || chapters.windows(2).any(|w| w[0] >= w[1])
    {
        return None;
    }
    let offers = chapters
        .into_iter()
        .zip(versions)
        .zip(prices)
        .map(|((chapter_id, version_id), price)| {
            Some(BatchChapterOffer {
                edition: ChapterVersionRef {
                    chapter_id: PositiveId::new(chapter_id).ok()?,
                    version_id: PositiveId::new(version_id).ok()?,
                },
                expected_price: MoneyMinor::new(price).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    BatchPurchaseCommand::new(
        PositiveId::new(actor).ok()?,
        WalletId::new(actor).ok()?,
        offers,
        IdempotencyKey::parse(fields.get("command_key")?).ok()?,
    )
    .ok()
}

fn failure(book_id: i64, error: BatchCheckoutError) -> Response {
    let (code, description) = match error {
        BatchCheckoutError::Checkout(CheckoutError::InsufficientBalance) => (
            StatusCode::CONFLICT,
            "Недостаточно средств во внутреннем кошельке. Списание не выполнено.",
        ),
        BatchCheckoutError::Checkout(CheckoutError::StaleOffer) => (
            StatusCode::CONFLICT,
            "Цена или редакция изменилась. Обновите выбор: списание не выполнено.",
        ),
        BatchCheckoutError::Checkout(CheckoutError::AlreadyOwned) => (
            StatusCode::CONFLICT,
            "Одна из глав уже куплена. Обновите выбор: повторного списания нет.",
        ),
        BatchCheckoutError::Checkout(CheckoutError::Inaccessible) => (
            StatusCode::NOT_FOUND,
            "Одна из глав недоступна. Покупка отменена целиком.",
        ),
        BatchCheckoutError::Checkout(error) => (
            checkout_status(error),
            "Не удалось подтвердить покупку. Проверьте выбор и состояние кошелька.",
        ),
        BatchCheckoutError::InvalidBatch | BatchCheckoutError::MixedBooks => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "Нужно выбрать от 2 до 20 разных глав одной книги.",
        ),
    };
    private_response(
        (
            code,
            shell(
                "Покупка не выполнена",
                book_id,
                &format!("<p role=\"alert\">{description}</p>"),
            ),
        ),
        &[],
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !state.paid_checkout_enabled || book_id <= 0 {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.reader.public_book_page(book_id as u64, 1, None).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let offers = match state.checkout.selectable_batch_offers(actor, book_id).await {
        Ok(offers) => offers,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(select_page(book_id, &book.title, &csrf, &offers), &cookies)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Response {
    if !state.paid_checkout_enabled || book_id <= 0 {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if !state.cookies.same_origin(&headers)
        || !fields
            .get("csrf_token")
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    match fields.get("step").map(String::as_str) {
        Some("select") => {
            let Some(ids) = selected(&fields) else {
                return deny(StatusCode::UNPROCESSABLE_ENTITY);
            };
            let book = match state.reader.public_book_page(book_id as u64, 1, None).await {
                Ok(Some(book)) => book,
                Ok(None) => return deny(StatusCode::NOT_FOUND),
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            };
            let available = match state.checkout.selectable_batch_offers(actor, book_id).await {
                Ok(available) => available,
                Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
            };
            let picked = ids
                .iter()
                .map(|id| {
                    available
                        .iter()
                        .find(|offer| offer.chapter_id == *id)
                        .cloned()
                })
                .collect::<Option<Vec<_>>>();
            let Some(picked) = picked else {
                return failure(
                    book_id,
                    BatchCheckoutError::Checkout(CheckoutError::StaleOffer),
                );
            };
            let Some(confirm) = confirm_page(
                book_id,
                &book.title,
                fields
                    .get("csrf_token")
                    .map(String::as_str)
                    .unwrap_or_default(),
                &picked,
            ) else {
                return deny(StatusCode::UNPROCESSABLE_ENTITY);
            };
            private_response(confirm, &[])
        }
        Some("commit") => {
            let Some(command) = command(actor, &fields) else {
                return deny(StatusCode::UNPROCESSABLE_ENTITY);
            };
            match state.checkout.checkout_batch_once(&command).await {
                Ok(receipt) => {
                    // Never display a success if even one purchase was rejected.
                    let mut links = String::new();
                    if state.paid_reader_available {
                        for item in &receipt.purchases {
                            write!(
                                links,
                                "<li><a href=\"/read/paid/{}?purchase_id={}\">Читать купленную главу {}</a></li>",
                                item.pinned.chapter_id.get(),
                                item.purchase_id.get(),
                                item.pinned.chapter_id.get(),
                            )
                            .expect("rendering HTML into String");
                        }
                    }
                    let reader_note = if state.paid_reader_available {
                        format!("<ol>{links}</ol><p><a href=\"/me/purchases\">Мои покупки</a></p>")
                    } else {
                        "<p>Чтение купленных редакций доступно при включённой защищённой читалке.</p>".to_owned()
                    };
                    private_response(
                        shell(
                            "Покупка завершена",
                            book_id,
                            &format!(
                                "<p role=\"status\">{} глав куплено одним действием. Из внутреннего кошелька списано {}.</p>
                                 <p>Остаток: {}. {}</p>{reader_note}
                                 <p><a href=\"/me/library\">Перейти в библиотеку</a></p>",
                                receipt.purchases.len(),
                                money(receipt.total.get()),
                                money(receipt.balance_after.get()),
                                if receipt.replayed { "Повтор без нового списания." } else { "" },
                            ),
                        ),
                        &[],
                    )
                }
                Err(error) => failure(book_id, error),
            }
        }
        _ => deny(StatusCode::UNPROCESSABLE_ENTITY),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_bounded_and_rejects_extra_fields() {
        let mut fields = HashMap::from([
            ("step".to_owned(), "select".to_owned()),
            ("csrf_token".to_owned(), "token".to_owned()),
            ("chapter_9".to_owned(), "1".to_owned()),
            ("chapter_8".to_owned(), "1".to_owned()),
        ]);
        assert_eq!(selected(&fields), Some(vec![8, 9]));
        fields.insert("user_id".to_owned(), "17".to_owned());
        assert_eq!(selected(&fields), None);
        fields.remove("user_id");
        fields.remove("chapter_8");
        assert_eq!(selected(&fields), None);
    }

    #[test]
    fn submitted_quote_is_bound_to_prices_and_rejects_duplicates() {
        let mut fields = HashMap::from([
            ("step".to_owned(), "commit".to_owned()),
            ("csrf_token".to_owned(), "token".to_owned()),
            ("command_key".to_owned(), format!("{}8", "A".repeat(42))),
            ("chapter_ids".to_owned(), "11,12".to_owned()),
            ("version_ids".to_owned(), "22,23".to_owned()),
            ("prices".to_owned(), "100,200".to_owned()),
        ]);
        assert_eq!(command(1, &fields).unwrap().total().get(), 300);
        fields.insert("chapter_ids".to_owned(), "11,11".to_owned());
        assert!(command(1, &fields).is_none());
        fields.insert("chapter_ids".to_owned(), "11,12".to_owned());
        fields.insert("wallet_id".to_owned(), "999".to_owned());
        assert!(command(1, &fields).is_none());
    }

    #[test]
    fn markup_escapes_titles_and_never_serializes_paid_bodies() {
        let offers = vec![
            SelectableBatchOffer {
                chapter_id: 5,
                chapter_no: 1,
                title: "<script>bad</script>".to_owned(),
                version_id: 11,
                price_minor: 125,
            },
            SelectableBatchOffer {
                chapter_id: 6,
                chapter_no: 2,
                title: "Вторая".to_owned(),
                version_id: 12,
                price_minor: 225,
            },
        ];
        let html = select_page(3, "Книга", "csrf", &offers).0;
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!html.contains("<script>bad</script>"));
        assert!(!html.contains("name=\"version_ids\""));
        let confirmation = confirm_page(3, "Книга", "csrf", &offers).unwrap().0;
        assert!(confirmation.contains("Итого: 3,50 ₽"));
        assert!(confirmation.contains("Подтвердить покупку"));
        assert!(confirmation.contains("name=\"prices\" value=\"125,225\""));
    }
}
