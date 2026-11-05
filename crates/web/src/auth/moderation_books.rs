//! A-05 / NEwBOO moderation: bounded book queue and reversible hold only.
//! Never publish a draft, grant paid chapter access, or edit owner metadata.
use super::{deny, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
#[path = "moderation_registration.rs"]
mod registration;
use tor_accounts::{
    repositories::moderation_books::{
        ModerationBook, ModerationBooksError, ModerationDeletionRequest,
    },
    services::security::session_token_digest,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    before: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HoldForm {
    csrf_token: String,
    expected_hold: bool,
    new_hold: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VisibilityForm {
    csrf_token: String,
    expected_status: String,
    expected_revision: i64,
    new_status: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeletionReviewForm {
    csrf_token: String,
    request_revision: i64,
    decision: String,
    #[serde(default)]
    reason: String,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/moderation/books", get(home))
        .route("/moderation/books/control", get(page))
        .route("/moderation/books/{id}/hold", post(submit))
        .route("/moderation/books/{id}/visibility", post(submit_visibility))
        .route("/moderation/books/deletions", get(deletion_queue))
        .route("/moderation/books/{id}/deletion", post(review_deletion))
        .merge(registration::router())
}

pub(super) fn error_code(error: ModerationBooksError) -> StatusCode {
    match error {
        ModerationBooksError::Denied => StatusCode::FORBIDDEN,
        ModerationBooksError::Missing => StatusCode::NOT_FOUND,
        ModerationBooksError::Invalid => StatusCode::BAD_REQUEST,
        ModerationBooksError::Stale => StatusCode::CONFLICT,
        ModerationBooksError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn render(rows: &[ModerationBook], csrf: &str) -> String {
    let mut list = String::new();
    for row in rows {
        let title = pages::escape_html(&row.title);
        let current = if row.admin_hold {
            "Удерживается модерацией"
        } else {
            "Без удержания"
        };
        let action = if row.admin_hold {
            "Снять удержание"
        } else {
            "Удержать"
        };
        let next = !row.admin_hold;
        let owner = row
            .owner_user_id
            .map_or_else(|| "—".to_owned(), |id| id.to_string());
        let registration_reviewer = match (row.approved_by_user_id, row.approved_by_name.as_deref())
        {
            (Some(id), Some(name)) => format!(
                "Первичную регистрацию одобрил {} (#{id})",
                pages::escape_html(name)
            ),
            _ => "Первоначальное решение не установлено".to_owned(),
        };
        list.push_str(&format!(
            r#"<article class="me-now-card">
               <h2>{title}</h2><p>Книга #{id} · Владелец #{owner} ·
               Публикация: {visibility} · {current}</p>
               <p>{registration_reviewer}
                 · <a href="/moderation/books/{id}/registration-history">
                 Системная история регистрации</a></p>
               <form method="post" action="/moderation/books/{id}/hold">
               <input type="hidden" name="csrf_token" value="{csrf}">
               <input type="hidden" name="expected_hold" value="{held}">
               <input type="hidden" name="new_hold" value="{next}">
               <button type="submit">{action}</button>
               </form></article>"#,
            id = row.id,
            visibility = pages::escape_html(&row.visibility),
            held = row.admin_hold,
        ));
        let mut choices = String::new();
        for (value, label) in [
            ("active", "Активна"),
            ("hidden", "Скрыта"),
            ("pending_moderation", "На модерации"),
            ("blocked", "Заблокирована"),
        ] {
            let selected = if row.visibility_status == value {
                " selected"
            } else {
                ""
            };
            choices.push_str(&format!(
                r#"<option value="{value}"{selected}>{label}</option>"#
            ));
        }
        list.push_str(&format!(
            r#"<form method="post" action="/moderation/books/{id}/visibility">
               <input type="hidden" name="csrf_token" value="{csrf}">
               <input type="hidden" name="expected_status" value="{current}">
               <input type="hidden" name="expected_revision" value="{revision}">
               <label for="visibility-status-{id}">Видимость на сайте</label>
               <select id="visibility-status-{id}" name="new_status">{choices}</select>
               <button type="submit">Сохранить видимость</button></form>"#,
            id = row.id,
            current = pages::escape_html(&row.visibility_status),
            revision = row.visibility_status_revision,
        ));
    }
    if rows.is_empty() {
        list.push_str("<p>Книги отсутствуют.</p>");
    }
    if rows.len() == 40 {
        if let Some(last) = rows.last() {
            list.push_str(&format!(
                r#"<a href="/moderation/books/control?before={}">Следующая страница</a>"#,
                last.id
            ));
        }
    }
    pages::layout(
        "Модерация книг",
        "Контроль видимости и удержаний без изменения публикации и покупок.",
        &format!(
            r#"<section class="me-page"><div class="container">
              <h1>Контроль книг модератором</h1>
              <nav aria-label="Очереди модератора">
                <a href="/moderation/books/registrations">Регистрация</a>
                <a href="/moderation/covers">Обложки</a>
                <a href="/moderation/books/deletions">Удаление</a>
                <a href="/moderation/books/control" aria-current="page">Контроль</a>
              </nav>
              <p>Контроль видимости и удержаний — не подтверждение заявки.</p>
              <p>Удержание не меняет авторскую публикацию и не создаёт
              административное право на платные главы.</p>
              {list}</div></section>"#
        ),
    )
    .0
}

/// Open the workbench on real requests, never on emergency controls.
/// Gate the redirect itself by a live site-moderator session.
pub(super) async fn home(
    State(state): State<AuthState>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    private_response(Redirect::to("/moderation/books/registrations"), &[])
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let rows = match state.accounts.moderation_books(&digest, query.before).await {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(Html(render(&rows, &pages::escape_html(&csrf))), &cookies);
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<HoldForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state
        .accounts
        .moderate_book_hold(&digest, id, form.expected_hold, form.new_hold)
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/books/control"), &[]),
        Err(error) => deny(error_code(error)),
    }
}

pub(super) async fn submit_visibility(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<VisibilityForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state
        .accounts
        .moderate_book_visibility(
            &digest,
            id,
            form.expected_revision,
            &form.expected_status,
            &form.new_status,
        )
        .await
    {
        Ok(_) => private_response(Redirect::to("/moderation/books/control"), &[]),
        Err(error) => deny(error_code(error)),
    }
}

fn render_deletion_queue(
    rows: &[ModerationDeletionRequest],
    csrf: &str,
    registration_count: i64,
    cover_count: i64,
    deletion_count: i64,
) -> String {
    let token = pages::escape_html(csrf);
    let mut cards = String::new();
    for item in rows {
        let title = pages::escape_html(&item.title);
        cards.push_str(&format!(
            r#"<article class="me-now-card"><h2>{title}</h2>
               <p>Книга #{book} · Владелец #{owner} · Заявка #{revision}</p>
               <form method="post" action="/moderation/books/{book}/deletion">
                 <input type="hidden" name="csrf_token" value="{token}">
                 <input type="hidden" name="request_revision" value="{revision}">
                 <label for="deletion-reason-{book}">Причина отказа (обязательна для отказа)</label>
                 <input id="deletion-reason-{book}" name="reason" maxlength="2000">
                 <button type="submit" name="decision" value="approve">Одобрить и удалить</button>
                 <button type="submit" name="decision" value="reject">Отклонить удаление</button>
               </form></article>"#,
            book = item.book_id,
            owner = item.owner_user_id,
            revision = item.request_revision
        ));
    }
    if cards.is_empty() {
        cards.push_str("<p>Ожидающих решения заявок нет.</p>");
    }
    if rows.len() == 40 {
        if let Some(last) = rows.last() {
            cards.push_str(&format!(
                "<p><a href=\"/moderation/books/deletions?before={}\">Следующая страница</a></p>",
                last.book_id
            ));
        }
    }
    pages::layout(
        "Модерация удаления книг",
        "Одобрение заявок на обычное удаление книги.",
        &format!(
            r#"<section class="me-page"><div class="container">
               <h1>Кабинет модератора</h1>
               <nav aria-label="Очереди модератора">
                 <a href="/moderation/books/registrations">Регистрация ({registration_count})</a>
                 <a href="/moderation/covers">Обложки ({cover_count})</a>
                 <a href="/moderation/books/deletions" aria-current="page">Удаление ({deletion_count})</a>
                 <a href="/moderation/books/control">Контроль</a>
               </nav>
               <h2>Заявки на удаление книг</h2>
               <p>Это решение не удаляет главы и не предоставляет платные права.
                  После одобрения книга удаляется из публичного доступа сразу.
                  Купленные и сохранённые главы остаются в личных библиотеках.</p>
               {cards}<p><a href="/moderation/books/control">Контроль и удержания книг</a></p>
               </div></section>"#
        ),
    )
    .0
}

pub(super) async fn deletion_queue(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let registration_count = match state
        .accounts
        .moderation_registration_pending_count(&digest)
        .await
    {
        Ok(count) => count,
        Err(error) => return deny(error_code(error)),
    };
    let cover_count = match tor_books::repositories::cover_review::PgCoverReview::new(
        state.accounts.pool().clone(),
    )
    .pending_count(&digest)
    .await
    {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let deletion_count = match state
        .accounts
        .moderation_pending_deletion_count(&digest)
        .await
    {
        Ok(count) => count,
        Err(error) => return deny(error_code(error)),
    };
    let rows = match state
        .accounts
        .moderation_pending_deletions(&digest, query.before)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        Html(render_deletion_queue(
            &rows,
            &csrf,
            registration_count,
            cover_count,
            deletion_count,
        )),
        &cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn review_deletion(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DeletionReviewForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state
        .accounts
        .moderate_ordinary_deletion(
            &digest,
            id,
            form.request_revision,
            &form.decision,
            &form.reason,
        )
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/books/deletions"), &[]),
        Err(error) => deny(error_code(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn moderation_list_escapes_private_titles_and_keeps_author_publication() {
        let rows = vec![ModerationBook {
            id: 42,
            title: "<script>private</script>".into(),
            visibility: "draft".into(),
            visibility_status: "pending_moderation".into(),
            visibility_status_revision: 2,
            admin_hold: false,
            owner_user_id: None,
            approved_by_user_id: Some(17),
            approved_by_name: Some("<script>reviewer</script>".into()),
        }];
        let html = render(&rows, "csrf");
        assert!(html.contains("&lt;script&gt;private&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("Первичную регистрацию одобрил"));
        assert!(html.contains("&lt;script&gt;reviewer&lt;/script&gt;"));
        assert!(html.contains("/moderation/books/42/registration-history"));
        assert!(!html.contains("value=\"draft\""));
        assert!(html.contains("name=\"new_hold\" value=\"true\""));
        assert!(!html.contains("name=\"visibility\""));
        assert!(!html.contains("purchase_id"));
        assert!(html.contains("name=\"expected_status\" value=\"pending_moderation\""));
        assert!(html.contains("name=\"expected_revision\" value=\"2\""));
        assert!(html.contains("/moderation/books/42/visibility"));
        assert!(!html.contains("name=\"access_level\""));
    }
}

#[cfg(test)]
mod deletion_queue_tests {
    use super::*;

    #[test]
    fn escapes_titles_and_never_trusts_client_actor_or_book_state() {
        let rows = vec![ModerationDeletionRequest {
            book_id: 42,
            title: "<script>alert(1)</script>".into(),
            owner_user_id: 7,
            request_revision: 2,
        }];
        let page = render_deletion_queue(&rows, "csrf", 1, 2, 3);
        assert!(page.contains("&lt;script&gt;"));
        assert!(!page.contains("<script>"));
        assert!(page.contains("/moderation/books/42/deletion"));
        assert!(page.contains("Регистрация (1)"));
        assert!(page.contains("Обложки (2)"));
        assert!(page.contains("Удаление (3)"));
        assert!(page.contains("value=\"approve\""));
        assert!(page.contains("value=\"reject\""));
        assert!(page.contains("name=\"request_revision\" value=\"2\""));
        let response = private_response(Html(page), &[]);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8",
            "moderator must receive parsed HTML, not text/plain"
        );
    }
}
