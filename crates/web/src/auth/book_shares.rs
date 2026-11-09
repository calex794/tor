//! S016/S017: direct friend book shares and recipient-only shared library.
use crate::{
    auth_cookies::{private_response, CookiePolicy},
    pages::{escape_html, layout},
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, Query, State},
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
use tor_social::repositories::book_shares::{BookShareError, PgBookShares};
#[path = "book_share_reports.rs"]
mod admin_reports;
#[path = "book_comment_appeals.rs"]
mod comment_appeals;
#[path = "book_content_reports.rs"]
mod content_reports;
#[path = "book_free_comment_reports.rs"]
mod free_comment_reports;
#[path = "book_free_comment_reviews.rs"]
mod free_comment_reviews;
#[path = "book_paid_comment_reviews.rs"]
mod paid_comment_reviews;

#[derive(Clone)]
struct ShareState {
    accounts: PgAccounts,
    shares: PgBookShares,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendForm {
    recipient_user_id: i64,
    message: String,
    command_key: String,
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionForm {
    action: String,
    reason: String,
    details: String,
    csrf_token: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharePageQuery {
    sent: Option<String>,
}

pub(super) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let shares = PgBookShares::new(accounts.pool().clone());
    let state = ShareState {
        accounts,
        shares,
        cookies,
    };
    Router::new()
        .route("/books/{id}/share", get(share_page).post(send))
        .route("/library/shared", get(inbox))
        .route("/library/shared/{share_id}/action", post(receiver_action))
        .route(
            "/books/{id}/report",
            get(content_reports::form).post(content_reports::submit),
        )
        .route(
            "/reader/{chapter_id}/comments/{comment_id}/report",
            get(free_comment_reports::form).post(free_comment_reports::submit),
        )
        .route("/admin/book-reports", get(content_reports::queue))
        .route(
            "/admin/book-reports/{report_id}/decision",
            post(content_reports::decide),
        )
        .route("/admin/recommendation-reports", get(admin_reports::queue))
        .route(
            "/admin/recommendation-reports/{share_id}/decision",
            post(admin_reports::decide),
        )
        .route("/me/comment-appeals", get(comment_appeals::author_page))
        .route(
            "/me/comment-appeals/{hide_event_id}",
            post(comment_appeals::submit),
        )
        .route(
            "/admin/free-comment-reports",
            get(free_comment_reviews::queue),
        )
        .route(
            "/admin/free-comment-reports/{report_id}/decision",
            post(free_comment_reviews::decide),
        )
        .route(
            "/admin/paid-comment-reports",
            get(paid_comment_reviews::queue),
        )
        .route(
            "/admin/paid-comment-reports/{report_id}/decision",
            post(paid_comment_reviews::decide),
        )
        .route("/admin/book-comment-appeals", get(comment_appeals::queue))
        .route(
            "/admin/book-comment-appeals/{appeal_id}/decision",
            post(comment_appeals::decide),
        )
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(state)
}

fn status(error: BookShareError) -> StatusCode {
    match error {
        BookShareError::Invalid => StatusCode::BAD_REQUEST,
        BookShareError::NotFound => StatusCode::NOT_FOUND,
        BookShareError::Forbidden => StatusCode::FORBIDDEN,
        BookShareError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        BookShareError::Conflict => StatusCode::CONFLICT,
        BookShareError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn deny(code: StatusCode) -> Response {
    private_response(code, &[])
}

fn page(title: &str, body: &str, cookies: &[String]) -> Response {
    let mut response = private_response(layout(title, "Рекомендации книг друзей", body), cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    response
}

async fn actor(s: &ShareState, h: &HeaderMap) -> Result<i64, StatusCode> {
    let raw = s.cookies.session_token(h).ok_or(StatusCode::UNAUTHORIZED)?;
    s.accounts
        .active_session_user(&session_token_digest(&raw))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn form_allowed(s: &ShareState, h: &HeaderMap, csrf: &str) -> bool {
    s.cookies.same_origin(h) && s.cookies.valid_csrf(h, csrf)
}

async fn share_page(
    State(s): State<ShareState>,
    Path(id): Path<i64>,
    Query(query): Query<SharePageQuery>,
    h: HeaderMap,
) -> Response {
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return deny(e),
    };
    let title = match s.shares.public_book_title(id, actor).await {
        Ok(title) => title,
        Err(e) => return deny(status(e)),
    };
    let friends = match s.shares.friends(actor).await {
        Ok(friends) => friends,
        Err(e) => return deny(status(e)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&h);
    let mut body = format!(
        "<section class=\"section\"><div class=\"container\"><h1>Посоветовать книгу другу</h1>
         <p>{}</p><p>Рекомендация не передаёт право на чтение платных глав. Для книг 18+/21+ получателю необходим действующий возрастной допуск.</p>",
        escape_html(&title)
    );
    if query.sent.as_deref() == Some("1") {
        body.push_str("<p role=\"status\">Рекомендация отправлена.</p>");
    }
    if friends.is_empty() {
        body.push_str("<p>Нет доступных друзей для отправки.</p>");
    } else {
        write!(
            body,
            r#"<form method="post" action="/books/{id}/share">
            <label>Кому<select name="recipient_user_id" required>"#
        )
        .expect("HTML");
        for friend in &friends {
            write!(
                body,
                r#"<option value="{}">{}</option>"#,
                friend.id,
                escape_html(&friend.name)
            )
            .expect("HTML");
        }
        write!(
            body,
            r#"</select></label>
            <label>Сообщение<textarea name="message" maxlength="500"></textarea></label>
            <input type="hidden" name="command_key" value="{}">
            <input type="hidden" name="csrf_token" value="{}">
            <button type="submit">Отправить рекомендацию</button></form>"#,
            generate_session_token(),
            escape_html(&csrf)
        )
        .expect("HTML");
    }
    body.push_str(
        r#"<p><a href="/library/shared">Полученные рекомендации</a></p></div></section>"#,
    );
    page("Поделиться книгой", &body, &cookies)
}

async fn send(
    State(s): State<ShareState>,
    Path(id): Path<i64>,
    h: HeaderMap,
    Form(f): Form<SendForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return deny(e),
    };
    match s
        .shares
        .send(actor, f.recipient_user_id, id, &f.message, &f.command_key)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/books/{id}/share?sent=1")), &[]),
        Err(e) => deny(status(e)),
    }
}

async fn inbox(State(s): State<ShareState>, h: HeaderMap) -> Response {
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return deny(e),
    };
    let shares = match s.shares.inbox(actor).await {
        Ok(shares) => shares,
        Err(e) => return deny(status(e)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&h);
    let mut body = String::from(
        r#"<section class="section"><div class="container">
        <h1>Рекомендации друзей</h1><p>Это ссылки на доступные книги, а не покупки или право на чтение.</p>"#,
    );
    if shares.is_empty() {
        body.push_str("<p>Рекомендаций пока нет.</p>");
    }
    for share in &shares {
        write!(
            body,
            r#"<article class="book-share"><h2><a href="/books/{}">{}</a></h2>
            <p>От: {}</p><p>{}</p>"#,
            share.book_id,
            escape_html(&share.book_title),
            escape_html(&share.sender_name),
            escape_html(&share.message)
        )
        .expect("HTML");
        for (action, label) in [("hide", "Скрыть"), ("mute", "Не получать от этого друга")]
        {
            write!(
                body,
                r#"<form method="post" action="/library/shared/{}/action">
                <input type="hidden" name="action" value="{}">
                <input type="hidden" name="reason" value="">
                <input type="hidden" name="details" value="">
                <input type="hidden" name="csrf_token" value="{}">
                <button type="submit">{}</button></form>"#,
                share.id,
                action,
                escape_html(&csrf),
                label
            )
            .expect("HTML");
        }
        write!(
            body,
            r#"<form method="post" action="/library/shared/{}/action">
            <input type="hidden" name="action" value="report">
            <label>Причина<select name="reason">
            <option value="spam">Спам</option><option value="abuse">Оскорбления</option>
            <option value="other">Другое</option></select></label>
            <label>Дополнение<input name="details" maxlength="500"></label>
            <input type="hidden" name="csrf_token" value="{}">
            <button type="submit">Пожаловаться</button></form></article>"#,
            share.id,
            escape_html(&csrf)
        )
        .expect("HTML");
    }
    body.push_str("</div></section>");
    page("Рекомендации друзей", &body, &cookies)
}

async fn receiver_action(
    State(s): State<ShareState>,
    Path(share_id): Path<i64>,
    h: HeaderMap,
    Form(f): Form<ActionForm>,
) -> Response {
    if !form_allowed(&s, &h, &f.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&s, &h).await {
        Ok(actor) => actor,
        Err(e) => return deny(e),
    };
    match s
        .shares
        .receiver_action(actor, share_id, &f.action, &f.reason, &f.details)
        .await
    {
        Ok(()) => private_response(Redirect::to("/library/shared"), &[]),
        Err(e) => deny(status(e)),
    }
}
