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
            "Текущая оферта: {},{:02} ₽; {}.",
            price / 100,
            price % 100,
            if kind == LayerTermKind::Lifetime {
                "бессрочно"
            } else {
                "30 дней, ручное продление"
            }
        ),
        _ => "Оферта ещё не опубликована.".to_owned(),
    };
    let price_value = offer.price_minor.map_or_else(String::new, |price| {
        format!("{},{:02}", price / 100, price % 100)
    });
    let options = if offer.term_kind == Some(LayerTermKind::Lifetime) {
        "<option value=\"lifetime\">Бессрочно</option>"
    } else {
        "<option value=\"prepaid_30d\">30 дней, только ручное продление</option>
         <option value=\"lifetime\">Бессрочно, включая будущие книги</option>"
    };
    format!(
        "<section aria-labelledby=\"layer-offer-heading\">
         <h2 id=\"layer-offer-heading\">Цена и условия Layer</h2><p>{old}</p>
         <p>Новая цена действует только для новых добровольных оплат.
         Старые оплаченные сроки не меняются, автосписаний нет.
         Для продажи нужен непустой состав и подтверждённые доли книг.</p>
         <form method=\"post\" action=\"/me/layers/{layer}/offer\">
         <label for=\"layer-price\">Цена в рублях</label>
         <input id=\"layer-price\" name=\"price_rub\" type=\"text\"
                inputmode=\"decimal\" placeholder=\"Например, 149,00\"
                pattern=\"[0-9]+([,.][0-9]{{1,2}})?\" required
                value=\"{price_value}\">
         <label for=\"layer-term\">Вид доступа</label>
         <select id=\"layer-term\" name=\"term_kind\">{options}</select>
         <input type=\"hidden\" name=\"expected_version_id\" value=\"{}\">
         <input type=\"hidden\" name=\"expected_offer_id\" value=\"{}\">
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
         <button class=\"button\" type=\"submit\">Опубликовать условия</button>
         </form></section>",
        offer.current_version_id,
        offer.current_offer_id.unwrap_or(0),
        pages::escape_html(csrf),
    )
}

#[cfg(test)]
fn picker_html(
    layer: i64,
    page: &LayerBookCandidatePage,
    current: &LayerCurrentBookPage,
    offer: &LayerOwnerOfferState,
    csrf: &str,
) -> axum::response::Html<String> {
    picker_html_with_query(layer, page, current, offer, csrf, "")
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
) -> axum::response::Html<String> {
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
            ).expect("write String");
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
            ).expect("write String");
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
    let candidates = match service
        .addable_books_matching(actor, layer, after, PAGE_SIZE, search)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => return deny(status(error)),
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
        picker_html_with_query(layer.get(), &candidates, &current, &offer, &csrf, search),
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
mod tests {
    use super::*;
    use tor_commerce::domain::layers::LayerBookCandidate;

    fn current_empty() -> LayerCurrentBookPage {
        LayerCurrentBookPage {
            current_version_id: 42,
            books: vec![],
            next_after: None,
        }
    }

    fn offer_empty() -> LayerOwnerOfferState {
        LayerOwnerOfferState {
            current_version_id: 42,
            current_offer_id: None,
            term_kind: None,
            price_minor: None,
        }
    }

    #[test]
    fn d_05_picker_escapes_untrusted_book_titles_and_excludes_user_identity() {
        let page = LayerBookCandidatePage {
            books: vec![LayerBookCandidate {
                book_id: 91,
                title: "<script>alert('x')</script>".into(),
            }],
            next_after: Some(91),
        };
        let html = picker_html(9, &page, &current_empty(), &offer_empty(), "test_csrf").0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("name=\"book_id\" value=\"91\""));
        assert!(html.contains("name=\"csrf_token\""));
        assert!(html.contains("books?after=91"));
        assert!(!html.contains("user_id"));
        assert!(!html.contains("share_percent"));
    }

    #[test]
    fn d_05_empty_picker_has_no_add_book_form() {
        let html = picker_html(
            9,
            &LayerBookCandidatePage {
                books: vec![],
                next_after: None,
            },
            &current_empty(),
            &offer_empty(),
            "token",
        )
        .0;
        assert!(html.contains("Подходящих книг нет"));
        assert!(!html.contains("name=\"book_id\""));
    }
    #[test]
    fn d_05_offer_form_is_cas_bound_and_never_contains_wallet_actor() {
        let mut offer = offer_empty();
        let html = offer_form_html(9, &offer, "<csrf>");
        assert!(html.contains("action=\"/me/layers/9/offer\""));
        assert!(html.contains("name=\"expected_version_id\" value=\"42\""));
        assert!(html.contains("name=\"expected_offer_id\" value=\"0\""));
        assert!(html.contains("&lt;csrf&gt;"));
        assert!(!html.contains("wallet_id"));
        assert!(!html.contains("user_id"));
        offer.current_offer_id = Some(31);
        offer.term_kind = Some(LayerTermKind::Lifetime);
        offer.price_minor = Some(14999);
        let html = offer_form_html(9, &offer, "csrf");
        assert!(html.contains("value=\"149,99\""));
        assert!(html.contains("name=\"expected_offer_id\" value=\"31\""));
        assert!(!html.contains("value=\"prepaid_30d\""));
    }

    #[test]
    fn d_05_current_layer_book_renders_version_bound_remove_form() {
        let page = LayerCurrentBookPage {
            current_version_id: 42,
            books: vec![tor_commerce::domain::layers::LayerBookCandidate {
                book_id: 91,
                title: "<img onerror=alert(1)>".into(),
            }],
            next_after: Some(91),
        };
        let html = picker_html(
            9,
            &LayerBookCandidatePage {
                books: vec![],
                next_after: None,
            },
            &page,
            &offer_empty(),
            "test_csrf",
        )
        .0;
        assert!(html.contains("name=\"operation\" value=\"remove\""));
        assert!(html.contains("name=\"expected_version_id\" value=\"42\""));
        assert!(html.contains("name=\"book_id\" value=\"91\""));
        assert!(html.contains("&lt;img onerror=alert(1)&gt;"));
        assert!(!html.contains("<img onerror="));
        assert!(html.contains("current_after=91"));
    }
}
