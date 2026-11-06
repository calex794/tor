//! MOD-COVER-01: the site moderator's private cover candidate queue.
//! Processed variants are not attached until an audited moderator decision.
use super::{deny, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tor_accounts::services::security::session_token_digest;
use tor_books::repositories::cover_review::{CoverReviewError, PendingCoverReview, PgCoverReview};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverQueueQuery {
    before: Option<i64>,
    q: Option<String>,
    kind: Option<String>,
    level: Option<String>,
}

/// Keep the queue's three optional filters together in the view contract.
struct CoverQueueFilters<'a> {
    search: Option<&'a str>,
    kind: Option<&'a str>,
    level: Option<&'a str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverDecisionForm {
    csrf_token: String,
    expected_slot_revision: i64,
    action: String,
    reason: Option<String>,
    confirmed: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/moderation/covers", get(page))
        .route("/moderation/covers/{id}/preview", get(preview))
        .route("/moderation/covers/{id}/decision", post(decide))
        .route("/moderation/books/{id}/cover-history", get(history))
}

fn error_code(err: CoverReviewError) -> StatusCode {
    match err {
        CoverReviewError::Denied => StatusCode::FORBIDDEN,
        CoverReviewError::Missing => StatusCode::NOT_FOUND,
        CoverReviewError::Invalid => StatusCode::BAD_REQUEST,
        CoverReviewError::Stale => StatusCode::CONFLICT,
        CoverReviewError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn session(state: &AuthState, headers: &HeaderMap) -> Option<[u8; 32]> {
    state
        .cookies
        .session_token(headers)
        .map(|token| session_token_digest(&token))
}

fn queue_body(
    items: &[PendingCoverReview],
    count: i64,
    csrf: &str,
    registration_count: i64,
    deletion_count: i64,
    filters: CoverQueueFilters<'_>,
) -> String {
    let search = pages::escape_html(filters.search.unwrap_or(""));
    let kind = filters.kind.unwrap_or("");
    let level = filters.level.unwrap_or("");
    let selected = |tier: &str| if level == tier { " selected" } else { "" };
    let level_all = selected("");
    let level_basic = selected("basic");
    let level_adult = selected("adult");
    let level_pro = selected("pro_adult");
    let level_legacy = selected("legacy");
    let first_selected = if kind == "first" { " selected" } else { "" };
    let replacement_selected = if kind == "replacement" {
        " selected"
    } else {
        ""
    };
    let all_selected = if kind.is_empty() { " selected" } else { "" };
    let mut cards = String::new();
    for item in items {
        let label = if item.expected_slot_revision == 0 {
            "Первая обложка"
        } else {
            "Замена обложки"
        };
        let title = pages::escape_html(&item.title);
        cards.push_str(&format!(
            r#"<article class="me-now-card"><h2>{title}</h2>
            <p>Книга #{book} · {label} · Кандидат #{review}
            · Загрузил пользователь #{actor}</p>
            <img src="/moderation/covers/{review}/preview"
                width="152" height="228" loading="lazy"
                alt="Приватный предпросмотр кандидата">
            <details class="moderation-review-confirm" open>
            <summary>Проверить обложку #{review}</summary>
            <p>Проверьте приватный предпросмотр. Одобрение заменит
               публичную обложку; отказ оставит прежнюю версию.</p>
            <form method="post" action="/moderation/covers/{review}/decision">
              <input type="hidden" name="csrf_token" value="{csrf}">
              <input type="hidden" name="expected_slot_revision" value="{rev}">
              <label for="reason-{review}">Причина отказа (обязательна при отклонении)</label>
              <textarea id="reason-{review}" name="reason" maxlength="2000" rows="2"></textarea>
              <button type="submit" name="action" value="approve">Продолжить одобрение</button>
              <button type="submit" name="action" value="reject">Продолжить отказ</button>
            </form></details>
            <p><a href="/moderation/books/{book}/cover-history">
              История решений по обложке</a>
              · <a href="/moderation/books/{book}/history">Все решения по книге</a></p>
            </article>"#,
            book = item.book_id,
            review = item.id,
            actor = item.submitted_by_user_id,
            rev = item.expected_slot_revision,
        ));
    }
    if items.is_empty() {
        cards.push_str("<p>Нет обложек на проверке.</p>");
    }
    if items.len() == 40 {
        if let Some(last) = items.last() {
            cards.push_str(&format!(
                "<form method=\"get\" action=\"/moderation/covers\">
                 <input type=\"hidden\" name=\"before\" value=\"{}\">
                 <input type=\"hidden\" name=\"q\" value=\"{search}\">
                 <input type=\"hidden\" name=\"kind\" value=\"{kind}\">
                 <input type=\"hidden\" name=\"level\" value=\"{level}\">
                 <button type=\"submit\">Следующая страница</button></form>",
                last.id
            ));
        }
    }
    pages::layout(
        "Модерация обложек",
        "Закрытая очередь первой обложки и замен.",
        &format!(
            r#"<section class="me-page"><div class="container">
              <h1>Модерация книг</h1>
              <nav aria-label="Очереди модератора">
                <a href="/moderation/books/registrations">Регистрация ({registration_count})</a>
                <a href="/moderation/covers" aria-current="page">Обложки ({count})</a>
                <a href="/moderation/books/deletions">Удаление ({deletion_count})</a>
                <a href="/moderation/books/control">Контроль</a>
              </nav>
              <p><a href="/moderation/books/reviews">Журнал рассмотренных заявок</a></p>
              <form method="get" action="/moderation/covers">
                <label for="cover-search">Поиск по названию или ID книги</label>
                <input id="cover-search" name="q" maxlength="120" value="{search}">
                <label for="cover-kind">Тип заявки</label>
                <select id="cover-kind" name="kind">
                  <option value=""{all_selected}>Все обложки</option>
                  <option value="first"{first_selected}>Первичная обложка</option>
                  <option value="replacement"{replacement_selected}>Замена обложки</option>
                </select>
                <label for="cover-level">Возрастная категория книги</label>
                <select id="cover-level" name="level">
                  <option value=""{level_all}>Все уровни</option>
                  <option value="basic"{level_basic}>Без ограничений</option>
                  <option value="adult"{level_adult}>18+</option>
                  <option value="pro_adult"{level_pro}>21+</option>
                  <option value="legacy"{level_legacy}>Не указан</option>
                </select>
                <button type="submit">Найти</button>
                <a href="/moderation/covers">Сбросить</a>
              </form>
              <p>Обработанное изображение не становится публичным без отдельного решения.</p>
              {cards}</div></section>"#
        ),
    )
    .0
}

pub(super) async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<CoverQueueQuery>,
) -> Response {
    let Some(digest) = session(&state, &headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let repo = PgCoverReview::new(state.accounts.pool().clone());
    let count = match repo.pending_count(&digest).await {
        Ok(count) => count,
        Err(error) => return deny(error_code(error)),
    };
    let registration_count = match state
        .accounts
        .moderation_registration_pending_count(&digest)
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
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let items = match repo
        .pending_with_access_level(
            &digest,
            query.before,
            query.q.as_deref(),
            query.kind.as_deref().filter(|kind| !kind.is_empty()),
            query.level.as_deref().filter(|level| !level.is_empty()),
        )
        .await
    {
        Ok(items) => items,
        Err(error) => return deny(error_code(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        Html(queue_body(
            &items,
            count,
            &pages::escape_html(&csrf),
            registration_count,
            deletion_count,
            CoverQueueFilters {
                search: query.q.as_deref(),
                kind: query.kind.as_deref(),
                level: query.level.as_deref(),
            },
        )),
        &cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn preview(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(digest) = session(&state, &headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let repo = PgCoverReview::new(state.accounts.pool().clone());
    let candidate = match repo.preview(&digest, id).await {
        Ok(Some(value)) => value,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(error_code(error)),
    };
    let Some(storage) = state.cover_preview_storage.as_ref() else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let data = match storage
        .get_bounded(&candidate.storage_key, 8 * 1024 * 1024)
        .await
    {
        Ok(data) => data,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let sha: [u8; 32] = Sha256::digest(&data).into();
    if sha != candidate.sha256 || data.len() as i64 != candidate.size_bytes {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    private_response(
        (
            [
                (header::CONTENT_TYPE, "image/webp"),
                (header::CACHE_CONTROL, "private, no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            data,
        ),
        &[],
    )
}

pub(super) async fn decide(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<CoverDecisionForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(digest) = session(&state, &headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let approve = match form.action.as_str() {
        "approve" => true,
        "reject" => false,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let confirmed = match super::moderation_confirm::was_confirmed(form.confirmed.as_deref()) {
        Ok(confirmed) => confirmed,
        Err(status) => return super::moderation_books::decision_error(status, "/moderation/covers"),
    };
    if !confirmed {
        let reason = form.reason.as_deref().unwrap_or("").trim();
        if id <= 0
            || form.expected_slot_revision < 0
            || reason.len() > 2000
            || (!approve && reason.is_empty())
        {
            return super::moderation_books::decision_error(StatusCode::BAD_REQUEST, "/moderation/covers");
        }
        return super::moderation_confirm::confirmation_page(
            super::moderation_confirm::ReviewIntent {
                kind: "cover",
                subject_id: id,
                revision: form.expected_slot_revision,
                action: &form.action,
                reason,
                csrf: &form.csrf_token,
            },
        );
    }
    let repo = PgCoverReview::new(state.accounts.pool().clone());
    match repo
        .decide(
            &digest,
            id,
            form.expected_slot_revision,
            approve,
            form.reason.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/covers"), &[]),
        Err(error) => {
            super::moderation_books::decision_error(error_code(error), "/moderation/covers")
        }
    }
}

pub(super) async fn history(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(digest) = session(&state, &headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let repo = PgCoverReview::new(state.accounts.pool().clone());
    let rows = match repo.private_history(&digest, book_id).await {
        Ok(rows) => rows,
        Err(error) => return deny(error_code(error)),
    };
    let mut entries = String::new();
    for row in rows {
        let reason = pages::escape_html(row.reason.as_deref().unwrap_or("—"));
        let moderator_name = pages::escape_html(&row.reviewer_name);
        entries.push_str(&format!(
            "<li>Заявка #{}, вариант #{}, одобрил/рассмотрел {moderator_name} (модератор #{}), решение {}, время Unix {}, причина: {reason}</li>",
            row.request_id,
            row.job_id,
            row.reviewer_user_id,
            pages::escape_html(&row.outcome),
            row.decided_unix,
        ));
    }
    let html = pages::layout(
        "История обложек",
        "Доступно только модерации сайта.",
        &format!(
            "<section class=\"me-page\"><div class=\"container\">
            <h1>История обложек книги #{book_id}</h1>
            <ol>{entries}</ol><a href=\"/moderation/covers\">Очередь обложек</a>
            </div></section>"
        ),
    );
    private_response(html, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cover_queue_escapes_titles_and_never_leaks_s3_keys() {
        let rows = vec![PendingCoverReview {
            id: 9,
            book_id: 2,
            title: "<script>bad</script>".into(),
            job_id: 38,
            submitted_by_user_id: 13,
            expected_slot_revision: 0,
        }];
        let html = queue_body(
            &rows,
            1,
            "csrf",
            2,
            3,
            CoverQueueFilters {
                search: None,
                kind: None,
                level: None,
            },
        );
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("/moderation/covers/9/preview"));
        assert!(html.contains("Первая обложка"));
        assert!(html.contains("moderation-review-confirm"));
        assert!(html.contains("name=\"kind\""));
        assert!(html.contains("name=\"level\""));
        assert!(html.contains("Регистрация (2)"));
        assert!(html.contains("Удаление (3)"));
        assert!(!html.contains("staging/covers/"));
        let response = private_response(Html(html), &[]);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8",
            "moderator queue must render functional forms in Chromium"
        );
    }
}
