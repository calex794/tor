//! S009: browser collection management. A collection is never a paid entitlement.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    pages::{escape_html, layout},
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_social::repositories::book_collections::{
    CollectionDetail, CollectionError, PgBookCollections,
};

#[derive(Clone)]
struct CollectionState {
    accounts: PgAccounts,
    collections: PgBookCollections,
    cookies: CookiePolicy,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateForm {
    title: String,
    visibility: String,
    request_key: String,
    csrf_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditForm {
    title: String,
    visibility: String,
    expected_revision: i64,
    csrf_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BookForm {
    book_id: i64,
    expected_revision: i64,
    csrf_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionForm {
    expected_revision: i64,
    csrf_token: String,
}

pub(super) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let collections = PgBookCollections::new(accounts.pool().clone());
    let state = CollectionState {
        accounts,
        collections,
        cookies,
    };
    Router::new()
        .route("/collections", get(index).post(create))
        .route("/collections/{id}", get(detail))
        .route("/collections/{id}/edit", post(edit))
        .route("/collections/{id}/books", post(add_book))
        .route(
            "/collections/{id}/books/{book_id}/remove",
            post(remove_book),
        )
        .route("/collections/{id}/delete", post(delete))
        .layer(DefaultBodyLimit::max(8 * 1024))
        .with_state(state)
}
async fn viewer(s: &CollectionState, h: &HeaderMap) -> Result<Option<i64>, StatusCode> {
    let Some(raw) = s.cookies.session_token(h) else {
        return Ok(None);
    };
    s.accounts
        .active_session_user(&session_token_digest(&raw))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
async fn actor(s: &CollectionState, h: &HeaderMap) -> Result<i64, StatusCode> {
    let raw = s.cookies.session_token(h).ok_or(StatusCode::UNAUTHORIZED)?;
    s.accounts
        .active_session_user(&session_token_digest(&raw))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)
}
fn status(error: CollectionError) -> StatusCode {
    match error {
        CollectionError::Invalid => StatusCode::BAD_REQUEST,
        CollectionError::NotFound => StatusCode::NOT_FOUND,
        CollectionError::Conflict => StatusCode::CONFLICT,
        CollectionError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}
fn failed(error: CollectionError) -> Response {
    private_response(status(error), &[])
}
fn denied(code: StatusCode) -> Response {
    private_response(code, &[])
}
fn page(title: &str, body: &str, cookies: &[String]) -> Response {
    let mut response = private_response(layout(title, "Подборки книг", body), cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    response
}
fn form_allowed(s: &CollectionState, h: &HeaderMap, csrf: &str) -> bool {
    s.cookies.same_origin(h) && s.cookies.valid_csrf(h, csrf)
}
async fn index(State(s): State<CollectionState>, h: HeaderMap) -> Response {
    let viewer = match viewer(&s, &h).await {
        Ok(v) => v,
        Err(code) => return denied(code),
    };
    let collections = match s.collections.list(viewer).await {
        Ok(v) => v,
        Err(e) => return failed(e),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&h);
    let mut html=String::from(
        "<section class=\"section\"><div class=\"container\"><h1>Подборки книг</h1>         <p>Подборка не открывает доступ к платным главам.</p><ul>"
    );
    for c in &collections {
        let visibility = if c.visibility == "private" {
            "Личная"
        } else {
            "Публичная"
        };
        write!(
            html,
            "<li><a href=\"/collections/{}\">{}</a> — {}</li>",
            c.id,
            escape_html(&c.title),
            visibility
        )
        .expect("HTML");
    }
    html.push_str("</ul>");
    if viewer.is_some() {
        write!(html,
            "<h2>Новая подборка</h2><form action=\"/collections\" method=\"post\">             <label>Название<input name=\"title\" required maxlength=\"160\"></label>             <label>Видимость<select name=\"visibility\">             <option value=\"private\">Личная</option>             <option value=\"public\">Публичная</option></select></label>             <input type=\"hidden\" name=\"request_key\" value=\"{}\">             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">             <button type=\"submit\">Создать</button></form>",
            generate_session_token(),escape_html(&csrf)
        ).expect("HTML");
    } else {
        html.push_str("<p><a href=\"/login\">Войти для создания</a></p>");
    }
    html.push_str("</div></section>");
    page("Подборки книг", &html, &cookies)
}
async fn detail(State(s): State<CollectionState>, Path(id): Path<i64>, h: HeaderMap) -> Response {
    let viewer = match viewer(&s, &h).await {
        Ok(v) => v,
        Err(code) => return denied(code),
    };
    let detail = match s.collections.detail(viewer, id).await {
        Ok(v) => v,
        Err(e) => return failed(e),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&h);
    page("Подборка книг", &detail_html(&detail, &csrf), &cookies)
}
fn detail_html(data: &CollectionDetail, csrf: &str) -> String {
    let c = &data.collection;
    let mut html=format!(
        "<section class=\"section\"><div class=\"container\"><h1>{}</h1>         <p>{}</p><p>{}</p><ul>",
        escape_html(&c.title),escape_html(&c.description),
        if c.visibility=="private" {"Личная подборка"} else {"Публичная подборка"}
    );
    for b in &data.books {
        write!(
            html,
            "<li><a href=\"/books/{}\">{}</a></li>",
            b.id,
            escape_html(&b.title)
        )
        .expect("HTML");
    }
    html.push_str("</ul>");
    if data.viewer_is_owner {
        let private_selected = if c.visibility == "private" {
            "selected"
        } else {
            ""
        };
        let public_selected = if c.visibility == "public" {
            "selected"
        } else {
            ""
        };
        write!(html,
            "<h2>Изменить подборку</h2>             <form method=\"post\" action=\"/collections/{}/edit\">             <input name=\"title\" required maxlength=\"160\" value=\"{}\">             <select name=\"visibility\"><option value=\"private\" {}>Личная</option>             <option value=\"public\" {}>Публичная</option></select>             <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">             <button type=\"submit\">Сохранить</button></form>             <h2>Добавить книгу</h2>             <form method=\"post\" action=\"/collections/{}/books\">             <label>ID публичной книги<input type=\"number\" name=\"book_id\" min=\"1\" required></label>             <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">             <button type=\"submit\">Добавить</button></form>",
            c.id,escape_html(&c.title),private_selected,public_selected,
            c.revision,escape_html(csrf),c.id,c.revision,escape_html(csrf)
        ).expect("HTML");
        for book in &data.books {
            write!(html,
                "<form method=\"post\" action=\"/collections/{}/books/{}/remove\">                 <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">                 <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">                 <button type=\"submit\">Убрать {}</button></form>",
                 c.id,book.id,c.revision,escape_html(csrf),escape_html(&book.title)
            ).expect("HTML");
        }
        write!(html,
            "<form method=\"post\" action=\"/collections/{}/delete\">             <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">             <button type=\"submit\">Удалить подборку</button></form>",
            c.id,c.revision,escape_html(csrf)
        ).expect("HTML");
    }
    html.push_str("<a href=\"/collections\">Все подборки</a></div></section>");
    html
}
async fn create(
    State(s): State<CollectionState>,
    h: HeaderMap,
    Form(f): Form<CreateForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return denied(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return denied(e),
    };
    match s
        .collections
        .create(actor, &f.title, &f.visibility, &f.request_key)
        .await
    {
        Ok(c) => private_response(Redirect::to(&format!("/collections/{}", c.id)), &[]),
        Err(e) => failed(e),
    }
}
async fn edit(
    State(s): State<CollectionState>,
    Path(id): Path<i64>,
    h: HeaderMap,
    Form(f): Form<EditForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return denied(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return denied(e),
    };
    match s
        .collections
        .update_metadata_if_current(actor, id, f.expected_revision, &f.title, &f.visibility)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/collections/{id}")), &[]),
        Err(e) => failed(e),
    }
}
async fn add_book(
    State(s): State<CollectionState>,
    Path(id): Path<i64>,
    h: HeaderMap,
    Form(f): Form<BookForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return denied(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return denied(e),
    };
    match s
        .collections
        .add_book(actor, id, f.expected_revision, f.book_id)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/collections/{id}")), &[]),
        Err(e) => failed(e),
    }
}
async fn remove_book(
    State(s): State<CollectionState>,
    Path((id, book_id)): Path<(i64, i64)>,
    h: HeaderMap,
    Form(f): Form<ActionForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return denied(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return denied(e),
    };
    match s
        .collections
        .remove_book_if_current(actor, id, f.expected_revision, book_id)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/collections/{id}")), &[]),
        Err(e) => failed(e),
    }
}
async fn delete(
    State(s): State<CollectionState>,
    Path(id): Path<i64>,
    h: HeaderMap,
    Form(f): Form<ActionForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return denied(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return denied(e),
    };
    match s.collections.delete_owned_collection(actor, id, f.expected_revision).await {
        Ok(_) => private_response(Redirect::to("/collections"), &[]),
        Err(e) => failed(e),
    }
}
