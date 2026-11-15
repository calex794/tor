//! D-05/H: owner-only Layer Book picker; no Layer checkout or payment UI.
//! The picker is advisory; D's locked add_book() rechecks membership on POST.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_commerce::{
    domain::layers::{
        LayerBookCandidatePage, LayerCurrentBookPage, LayerError, LayerOwnerOfferState,
        LayerTermKind,
    },
    repositories::layers::PgLayers,
    services::layers::LayerService,
};
use tor_contracts::paid_delivery::PositiveId;

const PAGE_SIZE: u16 = 20;

pub(super) fn routes() -> Router<AuthState> {
    Router::new().route("/me/layers/{layer_id}/books", get(page).post(add))
}

pub(super) fn enabled() -> bool {
    // Enabled for authors by default; operators may explicitly disable the feature.
    std::env::var("TOR_LAYER_MANAGEMENT_ENABLED").as_deref() != Ok("0")
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PickerQuery {
    after: Option<i64>,
    current_after: Option<i64>,
    q: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddBookForm {
    book_id: i64,
    csrf_token: Option<String>,
    operation: Option<String>,
    expected_version_id: Option<i64>,
    confirm_composition: Option<String>,
}

pub(super) fn status(error: LayerError) -> StatusCode {
    match error {
        LayerError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        LayerError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        LayerError::Inaccessible => StatusCode::NOT_FOUND,
        LayerError::StaleOffer | LayerError::InsufficientFunds | LayerError::ReplayConflict => {
            StatusCode::CONFLICT
        }
    }
}

fn offer_form_html(layer: i64, offer: &LayerOwnerOfferState, csrf: &str) -> String {
    let old = match (offer.price_minor, offer.term_kind) {
        (Some(price), Some(kind)) => format!(
            "Текущая цена: {},{:02} ₽; {}.",
            price / 100,
            price % 100,
            if kind == LayerTermKind::Lifetime {
                "бессрочно"
            } else {
                "30 дней, ручное продление"
            }
        ),
        _ => "Цена ещё не опубликована.".to_owned(),
    };
    let price_value = offer.price_minor.map_or_else(String::new, |price| {
        format!("{},{:02}", price / 100, price % 100)
    });
    let options = if offer.term_kind == Some(LayerTermKind::Lifetime) {
        r#"<option value="lifetime">Бессрочно, включая будущие книги</option>"#
    } else {
        r#"<option value="prepaid_30d">30 дней, только ручное продление</option>
            <option value="lifetime">Бессрочно, включая будущие книги</option>"#
    };
    format!(
        r#"<section aria-labelledby="layer-offer-heading">
         <h2 id="layer-offer-heading">Цена и условия пакета</h2>
         <p>{old}</p>
         <p>Цена действует только для новых добровольных покупок.
         Старые оплаченные сроки сохраняются, автоматических списаний нет.
         Для продажи необходим непустой состав и подтверждённые доли доходов.</p>
         <p class="author-layers-page__important"><strong>Важно о бессрочном доступе:</strong>
         покупатель получает права также на будущие книги, добавленные в этот пакет.
         Удаление книги из нового предложения не отменяет уже полученные права.
         Это долгосрочное обязательство перед читателем.</p>
         <form method="post" action="/me/layers/{layer}/offer" class="author-layer-card__form">
           <div class="author-layer-card__fields">
             <div class="author-layer-card__field">
               <label for="layer-price">Цена в рублях</label>
               <input id="layer-price" name="price_rub" type="text"
                      inputmode="decimal" placeholder="Например, 149,00"
                      pattern="[0-9]+([,.][0-9]{{1,2}})?" required
                      value="{price_value}">
             </div>
             <div class="author-layer-card__field">
               <label for="layer-term">Вид доступа</label>
               <select id="layer-term" name="term_kind">{options}</select>
             </div>
           </div>
           <label class="author-layers-page__confirm">
             <input type="checkbox" name="confirm_pricing" value="yes" required>
             Подтверждаю публикацию новых условий продажи без изменения предыдущих оплат
           </label>
           <label class="author-layers-page__confirm">
             <input type="checkbox" name="confirm_lifetime" value="yes">
             Если выбран бессрочный доступ: подтверждаю обязательство открывать покупателям и будущие книги пакета
           </label>
           <input type="hidden" name="expected_version_id" value="{}">
           <input type="hidden" name="expected_offer_id" value="{}">
           <input type="hidden" name="csrf_token" value="{}">
           <button class="ui-button ui-button--primary" type="submit">Опубликовать условия</button>
         </form></section>"#,
        offer.current_version_id,
        offer.current_offer_id.unwrap_or(0),
        pages::escape_html(csrf),
    )
}

fn encoded_search_suffix(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut encoded = String::new();
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("write String");
        }
    }
    format!("&amp;q={encoded}")
}

fn picker_html_with_query(
    layer: i64,
    page: &LayerBookCandidatePage,
    current: &LayerCurrentBookPage,
    offer: &LayerOwnerOfferState,
    csrf: &str,
    query: &str,
    editable: bool,
) -> axum::response::Html<String> {
    if !editable {
        let mut current_list = String::from("<ul class=\"author-layers-page__books\">");
        for book in &current.books {
            write!(
                current_list,
                "<li><span>{}</span></li>",
                pages::escape_html(&book.title),
            )
            .expect("write String");
        }
        current_list.push_str("</ul>");
        if current.books.is_empty() {
            current_list = String::from("<p>Книги пока не добавлены.</p>");
        }
        if let Some(cursor) = current.next_after {
            write!(
                current_list,
                r#"<p><a href="/me/layers/{layer}/books?current_after={cursor}">Следующие книги в пакете</a></p>"#,
            )
            .expect("write String");
        }
        let terms = match (offer.price_minor, offer.term_kind) {
            (Some(price), Some(kind)) => format!(
                "Последняя опубликованная цена: {},{:02} ₽; {}.",
                price / 100,
                price % 100,
                if kind == LayerTermKind::Lifetime {
                    "бессрочно"
                } else {
                    "30 дней"
                },
            ),
            _ => "Цена ещё не опубликована.".to_owned(),
        };
        let hint = "<p>Пакет скрыт от новых покупок. Состав и цену нельзя менять, пока он скрыт. Название, описание и видимость можно изменить в разделе «Мои пакеты».</p>";
        let content = include_str!("../../../templates/author_layer_books.html")
            .replace(
                "<!--ACCOUNT_NAVIGATION-->",
                &pages::account_navigation("/me/layers", csrf),
            )
            .replace("<!--CURRENT_BOOKS-->", &current_list)
            .replace("<!--BOOK_SEARCH-->", hint)
            .replace("<!--AVAILABLE_BOOKS-->", "")
            .replace(
                "<!--OFFER_FORM-->",
                &format!("<h2 id=\"layer-offer-heading\">Текущие условия</h2><p>{terms}</p>{hint}"),
            );
        return pages::private_account_layout(
            "Скрытый пакет книг",
            "Просмотр состава скрытого пакета без права изменения оплаченных условий.",
            &content,
        );
    }

    let suffix = encoded_search_suffix(query);
    let csrf = pages::escape_html(csrf);
    let mut current_list = String::new();
    if current.books.is_empty() {
        current_list.push_str("<p class=\"empty-state\">Книги пока не добавлены.</p>");
    } else {
        write!(
            current_list,
            r#"<form method="post" action="/me/layers/{layer}/books" class="author-layer-card__form">
            <input type="hidden" name="operation" value="remove">
            <input type="hidden" name="expected_version_id" value="{}">
            <input type="hidden" name="csrf_token" value="{csrf}">
            <ul class="author-layers-page__books">"#,
            current.current_version_id,
        ).expect("write String");
        for book in &current.books {
            write!(
                current_list,
                r#"<li><span>{}</span><button class="ui-button ui-button--ghost" type="submit"
                   name="book_id" value="{}">Убрать</button></li>"#,
                pages::escape_html(&book.title),
                book.book_id,
            )
            .expect("write String");
        }
        current_list.push_str(
            r#"</ul><label class="author-layers-page__confirm">
                <input type="checkbox" name="confirm_composition" value="yes" required>
                Подтверждаю изменение состава и сохранение ранее оплаченных прав
               </label></form>"#,
        );
    }
    if let Some(cursor) = current.next_after {
        write!(
            current_list,
            r#"<p><a class="text-link" href="/me/layers/{layer}/books?current_after={cursor}{suffix}">Следующие книги в пакете</a></p>"#,
        ).expect("write String");
    }

    let search = format!(
        r#"<form method="get" action="/me/layers/{layer}/books" class="author-layers-page__search">
          <label for="package-search">Найти свою книгу по названию</label>
          <input id="package-search" name="q" type="search" maxlength="100"
                 value="{}" placeholder="Введите название">
          <button class="ui-button ui-button--ghost" type="submit">Найти</button>
          <a class="text-link" href="/me/layers/{layer}/books">Сбросить</a>
        </form>"#,
        pages::escape_html(query),
    );

    let mut available = String::new();
    if page.books.is_empty() {
        available.push_str(
            "<p class=\"empty-state\">Подходящих книг не найдено. Проверьте запрос или состав команды книги.</p>",
        );
    } else {
        write!(
            available,
            r#"<form method="post" action="/me/layers/{layer}/books" class="author-layer-card__form">
               <input type="hidden" name="csrf_token" value="{csrf}">
               <ul class="author-layers-page__books">"#,
        ).expect("write String");
        for book in &page.books {
            write!(
                available,
                r#"<li><span>{}</span><button class="ui-button ui-button--ghost" type="submit"
                   name="book_id" value="{}">Добавить</button></li>"#,
                pages::escape_html(&book.title),
                book.book_id,
            )
            .expect("write String");
        }
        available.push_str(
            r#"</ul><label class="author-layers-page__confirm">
              <input type="checkbox" name="confirm_composition" value="yes" required>
              Подтверждаю изменение состава и возможное расширение прав действующих покупателей
              </label></form>"#,
        );
    }
    if let Some(cursor) = page.next_after {
        write!(
            available,
            r#"<p><a class="text-link" href="/me/layers/{layer}/books?after={cursor}{suffix}">Следующие книги</a></p>"#,
        ).expect("write String");
    }

    let content = include_str!("../../../templates/author_layer_books.html")
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &pages::account_navigation("/me/layers", &csrf),
        )
        .replace("<!--CURRENT_BOOKS-->", &current_list)
        .replace("<!--BOOK_SEARCH-->", &search)
        .replace("<!--AVAILABLE_BOOKS-->", &available)
        .replace("<!--OFFER_FORM-->", &offer_form_html(layer, offer, &csrf));
    pages::private_account_layout(
        "Книги и цена пакета",
        "Управление составом и условиями книжного пакета.",
        &content,
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    Query(query): Query<PickerQuery>,
    headers: HeaderMap,
) -> Response {
    if !enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    };
    let (Ok(actor), Ok(layer)) = (PositiveId::new(actor), PositiveId::new(raw_layer)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let after = match query.after.map(PositiveId::new).transpose() {
        Ok(after) => after,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    let search = query.q.as_deref().unwrap_or("").trim();
    if search.chars().count() > 100 {
        return deny(StatusCode::BAD_REQUEST);
    }
    let editable = match service.owner_layer_status(actor, layer).await {
        Ok(status) if status == "active" => true,
        Ok(status) if status == "hidden" => false,
        Ok(_) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    let candidates = if editable {
        match service
            .addable_books_matching(actor, layer, after, PAGE_SIZE, search)
            .await
        {
            Ok(candidates) => candidates,
            Err(error) => return deny(status(error)),
        }
    } else {
        LayerBookCandidatePage {
            books: Vec::new(),
            next_after: None,
        }
    };
    let current_after = match query.current_after.map(PositiveId::new).transpose() {
        Ok(after) => after,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    let current = match service
        .current_books(actor, layer, current_after, PAGE_SIZE)
        .await
    {
        Ok(current) => current,
        Err(error) => return deny(status(error)),
    };
    let offer = match service.owner_offer_state(actor, layer).await {
        Ok(offer) => offer,
        Err(error) => return deny(status(error)),
    };
    if offer.current_version_id != current.current_version_id {
        return deny(StatusCode::CONFLICT);
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        picker_html_with_query(
            layer.get(),
            &candidates,
            &current,
            &offer,
            &csrf,
            search,
            editable,
        ),
        &cookies,
    )
}

pub(super) async fn add(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<AddBookForm>,
) -> Response {
    if !enabled() {
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
    let (Ok(actor), Ok(layer), Ok(book)) = (
        PositiveId::new(actor),
        PositiveId::new(raw_layer),
        PositiveId::new(form.book_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    if form.confirm_composition.as_deref() != Some("yes") {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let service = LayerService::new(PgLayers::new(state.accounts.pool().clone()));
    let result = match form.operation.as_deref() {
        None | Some("add") if form.expected_version_id.is_none() => {
            service.add_book(actor, layer, book).await
        }
        Some("remove") => {
            let Some(Ok(expected)) = form.expected_version_id.map(PositiveId::new) else {
                return deny(StatusCode::UNPROCESSABLE_ENTITY);
            };
            service
                .remove_book_if_current(actor, layer, book, expected)
                .await
        }
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match result {
        Ok(_) => private_response(Redirect::to(&format!("/me/layers/{raw_layer}/books")), &[]),
        Err(error) => deny(status(error)),
    }
}

#[cfg(test)]
fn picker_html(
    layer: i64,
    page: &LayerBookCandidatePage,
    current: &LayerCurrentBookPage,
    offer: &LayerOwnerOfferState,
    csrf: &str,
) -> axum::response::Html<String> {
    picker_html_with_query(layer, page, current, offer, csrf, "", true)
}

#[cfg(test)]
mod tests;
