//! E-12 browser workflow. Only authenticated users may submit text corrections.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_discussions::repositories::text_issues::{
    CreatedIssue, IssueError, PgTextIssues, StatusMutation,
};

#[derive(Deserialize)]
pub(super) struct IssueForm {
    csrf_token: Option<String>,
    version_id: i64,
    block_index: i32,
    selection_start: i32,
    selection_end: i32,
    quote: String,
    suggested_text: String,
    issue_type: String,
}

#[derive(Default, Deserialize)]
pub(super) struct IssuesQuery {
    status: Option<String>,
    after: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct StatusForm {
    csrf_token: Option<String>,
    status: String,
    resolution_note: Option<String>,
}

fn status(error: IssueError) -> StatusCode {
    match error {
        IssueError::Invalid => StatusCode::BAD_REQUEST,
        IssueError::NotVisible => StatusCode::NOT_FOUND,
        IssueError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<IssueForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form.csrf_token.as_deref().is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(code) => return deny(code),
    };
    // Shared PG limiter rather than one in-memory counter per web instance.
    let key = session_token_digest(&format!("action:text_issue:user:{actor}"));
    let permitted = match state.accounts.permit_auth_attempt(&key, 20, 60).await {
        Ok(permitted) => permitted,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !permitted {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    }
    let repo = PgTextIssues::new(state.accounts.pool().clone());
    match repo.create_free(
        actor,
        chapter_id,
        form.version_id,
        form.block_index,
        form.selection_start,
        form.selection_end,
        &form.quote,
        &form.suggested_text,
        &form.issue_type,
    ).await {
        Ok(CreatedIssue::Added(_)) => private_response(
            Redirect::to(&format!("/read/{chapter_id}?issue=sent")), &[]
        ),
        Ok(CreatedIssue::Replayed(_)) => private_response(
            Redirect::to(&format!("/read/{chapter_id}?issue=repeat")), &[]
        ),
        Err(err) => deny(status(err)),
    }
}

pub(super) async fn author_page(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    Query(query): Query<IssuesQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(code) => return deny(code),
    };
    let repo = PgTextIssues::new(state.accounts.pool().clone());
    let page = match repo.author_page(
        actor, book_id, query.status.as_deref(), query.after
    ).await {
        Ok(Some(page)) => page,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(err) => return deny(status(err)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut cards = String::new();
    for issue in page.issues {
        let name = pages::escape_html(&issue.issue_type);
        let status_name = pages::escape_html(&issue.status);
        let quote = pages::escape_html(&issue.quote);
        let suggestion = pages::escape_html(&issue.suggested_text);
        let old_note = issue.resolution_note.as_deref().unwrap_or("");
        cards.push_str(&format!(
            r#"<section class="auth-card"><p>Глава №{chapter}, версия №{version}, фрагмент №{block}</p>
            <p>Тип: {name}. Состояние: {status_name}</p>
            <blockquote>{quote}</blockquote><p>Предложение: {suggestion}</p>
            <p><a href="/me/books/{book_id}/chapters/{chapter}/edit">Открыть редактор</a></p>
            <form method="post" action="/me/books/{book_id}/issues/{id}/status">
            <input type="hidden" name="csrf_token" value="{csrf}">
            <label>Состояние
              <select name="status">
                <option value="new">Новая</option>
                <option value="in_review">В работе</option>
                <option value="fixed">Исправлена</option>
                <option value="rejected">Отклонена</option>
              </select>
            </label>
            <label>Комментарий к решению
              <textarea name="resolution_note" maxlength="1000">{old_note}</textarea>
            </label>
            <button type="submit">Сохранить состояние</button></form></section>"#,
            chapter=issue.chapter_id,
            version=issue.version_id,
            block=issue.block_index,
            id=issue.id,
            csrf=pages::escape_html(&csrf),
            old_note=pages::escape_html(old_note)
        ));
    }
    let navigation = if let Some(after) = page.next {
        let filter = query.status.as_deref().map_or(String::new(), |v| format!("&amp;status={}", pages::escape_html(v)));
        format!("<p><a href=\"/me/books/{book_id}/issues?after={after}{filter}\">Следующая страница</a></p>")
    } else { String::new() };
    let body = format!(
        r#"<section class="section auth-section"><div class="container">
        <h1>Ошибки текста — {title}</h1>
        <nav><a href="/me/books/{book_id}">К книге</a> ·
        <a href="/me/books/{book_id}/issues">Все</a> ·
        <a href="/me/books/{book_id}/issues?status=new">Новые</a> ·
        <a href="/me/books/{book_id}/issues?status=in_review">В работе</a> ·
        <a href="/me/books/{book_id}/issues?status=fixed">Исправленные</a> ·
        <a href="/me/books/{book_id}/issues?status=rejected">Отклонённые</a></nav>
        {cards}{navigation}</div></section>"#,
        title=pages::escape_html(&page.book_title)
    );
    private_response(pages::layout("Ошибки текста", "Очередь исправлений автора.", &body), &cookies)
}

pub(super) async fn change_status(
    State(state): State<AuthState>,
    Path((book_id, issue_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<StatusForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form.csrf_token.as_deref().is_some_and(|csrf| state.cookies.valid_csrf(&headers,csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(code) => return deny(code),
    };
    let repo = PgTextIssues::new(state.accounts.pool().clone());
    match repo.update_status(
        actor, book_id, issue_id, &form.status,
        form.resolution_note.as_deref().filter(|value| !value.is_empty()),
    ).await {
        Ok(StatusMutation::Changed | StatusMutation::Replayed) =>
            private_response(Redirect::to(&format!("/me/books/{book_id}/issues")), &[]),
        Ok(StatusMutation::NotFound) => deny(StatusCode::NOT_FOUND),
        Err(err) => deny(status(err)),
    }
}
