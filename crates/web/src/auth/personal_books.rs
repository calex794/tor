//! S033 private personal materials. No public Book IDs, team ACL or sharing.
#![forbid(unsafe_code)]
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    pages::{escape_html, layout},
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{
    repositories::personal_books::PgPersonalBooks,
    services::personal_books::{
        PersonalBook, PersonalChapter, PersonalChapterSummary, PersonalError,
    },
};

#[derive(Clone)]
struct PersonalState {
    accounts: PgAccounts,
    books: PgPersonalBooks,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
struct BookForm {
    title: String,
    description: Option<String>,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct ChapterForm {
    title: String,
    content_markdown: Option<String>,
    csrf_token: Option<String>,
}

pub(super) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let state = PersonalState {
        books: PgPersonalBooks::new(accounts.pool().clone()),
        accounts,
        cookies,
    };
    Router::new()
        .route("/personal-books", get(index))
        .route("/personal-books/new", get(new_book).post(create_book))
        .route("/personal-books/{id}", get(book))
        .route(
            "/personal-books/{id}/edit",
            get(edit_book).post(update_book),
        )
        .route(
            "/personal-books/{id}/chapters/new",
            get(new_chapter).post(create_chapter),
        )
        .route(
            "/personal-books/{id}/chapters/{chapter_id}",
            get(read_chapter),
        )
        .route(
            "/personal-books/{id}/chapters/{chapter_id}/edit",
            get(edit_chapter).post(update_chapter),
        )
        .layer(DefaultBodyLimit::max(600 * 1024))
        .with_state(state)
}

async fn actor(state: &PersonalState, headers: &HeaderMap) -> Result<i64, Response> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Err(private_response(Redirect::to("/login"), &[]));
    };
    match state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
    {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(private_response(Redirect::to("/login"), &[])),
        Err(_) => Err(private_response(StatusCode::SERVICE_UNAVAILABLE, &[])),
    }
}
fn error(error: PersonalError) -> Response {
    let status = match error {
        PersonalError::Invalid => StatusCode::BAD_REQUEST,
        PersonalError::Missing => StatusCode::NOT_FOUND,
        PersonalError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    private_response(status, &[])
}
fn form_allowed(state: &PersonalState, headers: &HeaderMap, csrf: Option<&str>) -> bool {
    state.cookies.same_origin(headers) && state.cookies.valid_csrf(headers, csrf.unwrap_or(""))
}
fn display(title: &str, content: &str, cookies: &[String]) -> Response {
    let mut page = private_response(
        layout(title, "Личные материалы. Доступ только владельцу.", content),
        cookies,
    );
    page.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    page
}
fn page_form(
    title: &str,
    action: &str,
    csrf: &str,
    chapter: bool,
    name: &str,
    value: &str,
) -> String {
    let label = if chapter {
        "Текст главы"
    } else {
        "Описание"
    };
    let field = if chapter {
        "content_markdown"
    } else {
        "description"
    };
    let limit = if chapter { 500000 } else { 10000 };
    format!(
        "<section class=\"section auth-section\"><div class=\"container\"><h1>{}</h1>\
         <form method=\"post\" action=\"{}\">\
         <label>Название<input name=\"title\" maxlength=\"200\" required value=\"{}\"></label>\
         <label>{label}<textarea name=\"{field}\" maxlength=\"{limit}\">{}</textarea></label>\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
         <button class=\"button\" type=\"submit\">Сохранить</button></form>\
         <p><a href=\"/personal-books\">Личные книги</a></p></div></section>",
        escape_html(title),
        escape_html(action),
        escape_html(name),
        escape_html(value),
        escape_html(csrf)
    )
}
fn book_list(books: &[PersonalBook]) -> String {
    let mut out = String::from(
        "<section class=\"section\"><div class=\"container\"><h1>Личные книги</h1>\
         <p>Доступны только вам. Не публикуются в каталоге.</p>\
         <p><a class=\"button\" href=\"/personal-books/new\">Создать личную книгу</a></p><ul>",
    );
    for book in books {
        write!(
            out,
            "<li><a href=\"/personal-books/{}\">{}</a></li>",
            book.id,
            escape_html(&book.title)
        )
        .expect("HTML output");
    }
    out.push_str("</ul><a href=\"/me\">Мой кабинет</a></div></section>");
    out
}
fn book_details(book: &PersonalBook, chapters: &[PersonalChapterSummary]) -> String {
    let mut out = format!(
        "<section class=\"section\"><div class=\"container\"><h1>{}</h1><p>{}</p>\
         <p><a href=\"/personal-books/{}/edit\">Изменить книгу</a> · \
         <a href=\"/personal-books/{}/chapters/new\">Добавить главу</a></p><ol>",
        escape_html(&book.title),
        escape_html(&book.description),
        book.id,
        book.id,
    );
    for chapter in chapters {
        write!(
            out,
            "<li><a href=\"/personal-books/{}/chapters/{}\">{}. {}</a></li>",
            book.id,
            chapter.id,
            chapter.chapter_no,
            escape_html(&chapter.title)
        )
        .expect("HTML output");
    }
    out.push_str("</ol><a href=\"/personal-books\">Все личные книги</a></div></section>");
    out
}
fn chapter_detail(chapter: &PersonalChapter) -> String {
    format!(
        "<section class=\"section\"><div class=\"container\"><h1>{}</h1>\
         <pre style=\"white-space:pre-wrap;overflow-wrap:anywhere\">{}</pre>\
         <p><a href=\"/personal-books/{}/chapters/{}/edit\">Редактировать главу</a> · \
         <a href=\"/personal-books/{}\">Оглавление</a></p></div></section>",
        escape_html(&chapter.title),
        escape_html(&chapter.content_markdown),
        chapter.book_id,
        chapter.id,
        chapter.book_id
    )
}
async fn index(State(s): State<PersonalState>, headers: HeaderMap) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match s.books.list_books(actor).await {
        Ok(books) => display("Личные книги", &book_list(&books), &[]),
        Err(e) => error(e),
    }
}
async fn new_book(State(s): State<PersonalState>, headers: HeaderMap) -> Response {
    if let Err(e) = actor(&s, &headers).await {
        return e;
    }
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    display(
        "Новая личная книга",
        &page_form(
            "Новая личная книга",
            "/personal-books/new",
            &csrf,
            false,
            "",
            "",
        ),
        &cookies,
    )
}
async fn create_book(
    State(s): State<PersonalState>,
    headers: HeaderMap,
    Form(form): Form<BookForm>,
) -> Response {
    if !form_allowed(&s, &headers, form.csrf_token.as_deref()) {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match s
        .books
        .create_book(actor, &form.title, form.description.as_deref().unwrap_or(""))
        .await
    {
        Ok(book) => private_response(Redirect::to(&format!("/personal-books/{}", book.id)), &[]),
        Err(e) => error(e),
    }
}
async fn book(
    State(s): State<PersonalState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    let book = match s.books.book(actor, book_id).await {
        Ok(book) => book,
        Err(e) => return error(e),
    };
    let chapters = match s.books.list_chapters(actor, book_id).await {
        Ok(chapters) => chapters,
        Err(e) => return error(e),
    };
    display("Личная книга", &book_details(&book, &chapters), &[])
}
async fn edit_book(
    State(s): State<PersonalState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    let book = match s.books.book(actor, book_id).await {
        Ok(book) => book,
        Err(e) => return error(e),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    display(
        "Редактировать личную книгу",
        &page_form(
            "Редактировать личную книгу",
            &format!("/personal-books/{book_id}/edit"),
            &csrf,
            false,
            &book.title,
            &book.description,
        ),
        &cookies,
    )
}
async fn update_book(
    State(s): State<PersonalState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<BookForm>,
) -> Response {
    if !form_allowed(&s, &headers, form.csrf_token.as_deref()) {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match s
        .books
        .update_book(
            actor,
            book_id,
            &form.title,
            form.description.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/personal-books/{book_id}")), &[]),
        Err(e) => error(e),
    }
}
async fn new_chapter(
    State(s): State<PersonalState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    if let Err(e) = s.books.book(actor, book_id).await {
        return error(e);
    }
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    display(
        "Новая личная глава",
        &page_form(
            "Новая личная глава",
            &format!("/personal-books/{book_id}/chapters/new"),
            &csrf,
            true,
            "",
            "",
        ),
        &cookies,
    )
}
async fn create_chapter(
    State(s): State<PersonalState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ChapterForm>,
) -> Response {
    if !form_allowed(&s, &headers, form.csrf_token.as_deref()) {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match s
        .books
        .create_chapter(
            actor,
            book_id,
            &form.title,
            form.content_markdown.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(ch) => private_response(
            Redirect::to(&format!("/personal-books/{book_id}/chapters/{}", ch.id)),
            &[],
        ),
        Err(e) => error(e),
    }
}
async fn read_chapter(
    State(s): State<PersonalState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match s.books.chapter(actor, book_id, chapter_id).await {
        Ok(chapter) => display("Личная глава", &chapter_detail(&chapter), &[]),
        Err(e) => error(e),
    }
}
async fn edit_chapter(
    State(s): State<PersonalState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    let chapter = match s.books.chapter(actor, book_id, chapter_id).await {
        Ok(ch) => ch,
        Err(e) => return error(e),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    display(
        "Редактировать личную главу",
        &page_form(
            "Редактировать личную главу",
            &format!("/personal-books/{book_id}/chapters/{chapter_id}/edit"),
            &csrf,
            true,
            &chapter.title,
            &chapter.content_markdown,
        ),
        &cookies,
    )
}
async fn update_chapter(
    State(s): State<PersonalState>,
    Path((book_id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ChapterForm>,
) -> Response {
    if !form_allowed(&s, &headers, form.csrf_token.as_deref()) {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match actor(&s, &headers).await {
        Ok(id) => id,
        Err(e) => return e,
    };
    match s
        .books
        .update_chapter(
            actor,
            book_id,
            chapter_id,
            &form.title,
            form.content_markdown.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(_) => private_response(
            Redirect::to(&format!("/personal-books/{book_id}/chapters/{chapter_id}")),
            &[],
        ),
        Err(e) => error(e),
    }
}
