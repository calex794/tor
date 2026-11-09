//! S049: private browser form for a visible free-chapter edition comment.
//! Reports carry only an exact E identifier and a reason, never the chapter text.
use super::{deny, page, status, ShareState};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_social::repositories::free_comment_reports::PgFreeCommentReports;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportQuery {
    version_id: i64,
    sent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportForm {
    csrf_token: String,
    version_id: i64,
    command_key: String,
    reason: String,
    details: String,
}

async fn session_user(s: &ShareState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let token = s
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    s.accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(digest)
}

fn store(s: &ShareState) -> PgFreeCommentReports {
    PgFreeCommentReports::new(s.accounts.pool().clone())
}

pub(super) async fn form(
    State(s): State<ShareState>,
    Path((chapter_id, comment_id)): Path<(i64, i64)>,
    Query(query): Query<ReportQuery>,
    headers: HeaderMap,
) -> Response {
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(code) => return deny(code),
    };
    if let Err(error) = store(&s)
        .reportable(&digest, chapter_id, query.version_id, comment_id)
        .await
    {
        return deny(status(error));
    }
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let status = if query.sent.as_deref() == Some("1") {
        "<p role=\"status\">Жалоба зарегистрирована.</p>"
    } else {
        ""
    };
    let body = format!(
        r#"<section class="section"><div class="container">
        <h1>Жалоба на комментарий к бесплатной главе</h1>
        <p>Глава #{chapter_id} · опубликованная редакция #{} · комментарий #{comment_id}</p>
        <p>Жалоба не скрывает комментарий автоматически и не открывает платные редакции.</p>
        {status}<form method="post" action="/reader/{chapter_id}/comments/{comment_id}/report">
        <label>Причина <select name="reason" required>
        <option value="spam">Спам</option>
        <option value="abuse">Оскорбления или угрозы</option>
        <option value="misleading">Недостоверные сведения</option>
        <option value="other">Другое</option>
        </select></label>
        <label>Обоснование <textarea name="details" maxlength="500"></textarea></label>
        <input type="hidden" name="version_id" value="{}">
        <input type="hidden" name="csrf_token" value="{}">
        <input type="hidden" name="command_key" value="{}">
        <button type="submit">Отправить жалобу</button>
        </form>
        <p><a href="/reader/{chapter_id}#chapter-comments">Вернуться к обсуждению</a></p>
        </div></section>"#,
        query.version_id,
        query.version_id,
        super::escape_html(&csrf),
        generate_session_token(),
    );
    page("Жалоба на комментарий к главе", &body, &cookies)
}

pub(super) async fn submit(
    State(s): State<ShareState>,
    Path((chapter_id, comment_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ReportForm>,
) -> Response {
    if !s.cookies.same_origin(&headers) || !s.cookies.valid_csrf(&headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(code) => return deny(code),
    };
    match store(&s)
        .submit(
            &digest,
            (chapter_id, form.version_id, comment_id),
            &form.reason,
            &form.details,
            &form.command_key,
        )
        .await
    {
        Ok(_) => super::private_response(
            Redirect::to(&format!(
                "/reader/{chapter_id}/comments/{comment_id}/report?version_id={}&sent=1",
                form.version_id
            )),
            &[],
        ),
        Err(error) => deny(status(error)),
    }
}
