//! H/UI-19: owner-only controls for package-restricted NEW paid chapters.
//! Separate page avoids nested forms within the script-free composition grid.
use super::{deny, layer_books, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_commerce::repositories::{
    layer_package_sales::{PackagePaidMode, PackageSalePage},
    layers::PgLayers,
};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaleForm {
    book_id: i64,
    expected_version_id: i64,
    expected_revision: i64,
    mode: String,
    confirm: String,
    csrf_token: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaleQuery {
    updated: Option<String>,
}

pub(super) fn routes() -> Router<AuthState> {
    Router::new().route(
        "/me/layers/{layer_id}/sales",
        get(page).post(save).layer(DefaultBodyLimit::max(4096)),
    )
}

fn show_page(
    layer: i64,
    data: &PackageSalePage,
    csrf: &str,
    updated: bool,
) -> axum::response::Html<String> {
    let mut list = String::new();
    for book in &data.books {
        let title = pages::escape_html(&book.title);
        let status = if book.restricted_elsewhere {
            "Платные главы зарезервированы для другого пакета.".to_owned()
        } else if book.restricted_here {
            match &book.until_utc {
                Some(until) => format!(
                    "Ранний доступ до {until} UTC. После срока отдельные покупки откроются."
                ),
                None => {
                    "Постоянно: новые платные главы продаются только через этот пакет.".to_owned()
                }
            }
        } else {
            "Прямая продажа платных глав доступна.".to_owned()
        };
        let form = if data.editable && !book.restricted_elsewhere {
            format!(
                r#"<form method="post" action="/me/layers/{layer}/sales">
                    <input type="hidden" name="csrf_token" value="{csrf}">
                    <input type="hidden" name="book_id" value="{id}">
                    <input type="hidden" name="expected_version_id" value="{version}">
                    <input type="hidden" name="expected_revision" value="{revision}">
                    <label for="sales-mode-{id}">Условия новых покупок</label>
                    <select id="sales-mode-{id}" name="mode" required>
                        <option value="" selected disabled>Выберите новый режим</option>
                        <option value="direct">Обычная продажа</option>
                        <option value="7_days">Только пакет — первые 7 дней</option>
                        <option value="14_days">Только пакет — первые 14 дней</option>
                        <option value="package_only">Только пакет — постоянно</option>
                    </select>
                    <label><input type="checkbox" name="confirm" value="yes" required>
                        Подтверждаю изменение новых продаж. Оплаченные права читателей сохраняются.
                    </label>
                    <button class="ui-button ui-button--primary" type="submit">
                        Сохранить режим для книги
                    </button>
                </form>"#,
                csrf = pages::escape_html(csrf),
                id = book.id,
                version = data.version_id,
                revision = book.revision,
            )
        } else {
            "<p>Изменения недоступны, пока пакет не опубликован или книга закреплена за другим пакетом.</p>".to_owned()
        };
        write!(
            list,
            r#"<article class="me-now-card package-sale__book">
               <h3>{title}</h3><p>{status}</p>{form}
               </article>"#,
        )
        .expect("format package paid sales");
    }
    if list.is_empty() {
        list.push_str("<p>В пакете пока нет книг. Сначала добавьте книги в редакторе состава.</p>");
    }
    let notice = if updated {
        r#"<p class="ui-alert ui-alert--success" role="status">Режим новых продаж сохранён.</p>"#
    } else {
        ""
    };
    let disabled = if data.editable {
        ""
    } else {
        "<p>Пакет скрыт или не имеет опубликованной цены. Изменять режимы нельзя.</p>"
    };
    let content = format!(
        r#"<section class="me-page author-workspace"><div class="container">
            <p><a href="/me/layers/{layer}/books">К составу пакета</a></p>
            <h1>Ранний и эксклюзивный доступ</h1>
            <p>Режим касается <strong>только новых покупок платных глав</strong>.
               Бесплатные главы, все ранее оплаченные редакции и действующие права
               по пакетам остаются доступными по прежним правилам.</p>
            <p>Пока выбран режим «только пакет», книгу нельзя продавать отдельными
               главами или включать в другой пакет. Если пакет скрыть либо
               исключить из него книгу, ограничение автоматически снимается.</p>
            {notice}{disabled}{list}
          </div></section>"#,
    );
    pages::private_account_layout(
        "Управление продажами книг пакета",
        "Настройка раннего и эксклюзивного доступа к платным главам",
        &content,
    )
}

async fn page(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    Query(query): Query<SaleQuery>,
    headers: HeaderMap,
) -> Response {
    if !layer_books::enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(error) => return deny(error),
    };
    let (Ok(actor), Ok(layer)) = (PositiveId::new(actor), PositiveId::new(raw_layer)) else {
        return deny(StatusCode::NOT_FOUND);
    };
    let data = match PgLayers::new(state.accounts.pool().clone())
        .package_sale_page(actor, layer)
        .await
    {
        Ok(page) => page,
        Err(error) => return deny(layer_books::status(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        show_page(
            raw_layer,
            &data,
            &csrf,
            query.updated.as_deref() == Some("1"),
        ),
        &cookies,
    )
}

async fn save(
    State(state): State<AuthState>,
    Path(raw_layer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<SaleForm>,
) -> Response {
    if !layer_books::enabled() {
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
    let (Ok(actor), Ok(layer), Ok(book), Ok(version), Some(mode)) = (
        PositiveId::new(actor),
        PositiveId::new(raw_layer),
        PositiveId::new(form.book_id),
        PositiveId::new(form.expected_version_id),
        PackagePaidMode::parse(&form.mode),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    if form.confirm != "yes" || form.expected_revision < 0 {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    match PgLayers::new(state.accounts.pool().clone())
        .set_package_paid_mode(actor, layer, book, version, form.expected_revision, mode)
        .await
    {
        Ok(_) => private_response(
            Redirect::to(&format!("/me/layers/{raw_layer}/sales?updated=1")),
            &[],
        ),
        Err(error) => deny(layer_books::status(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn h_ui_19_page_displays_paid_only_notice_and_csrf() {
        let html = show_page(
            12,
            &PackageSalePage {
                version_id: 9,
                editable: true,
                books: vec![
                    tor_commerce::repositories::layer_package_sales::PackageSaleBook {
                        id: 41,
                        title: "<script>x</script>".into(),
                        revision: 2,
                        restricted_here: true,
                        restricted_elsewhere: false,
                        until_utc: None,
                    },
                ],
            },
            "safe-token",
            false,
        )
        .0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>x</script>"));
        assert!(html.contains("name=\"expected_revision\" value=\"2\""));
        assert!(html.contains("name=\"csrf_token\" value=\"safe-token\""));
        assert!(html.contains("только новых покупок платных глав"));
    }
}
