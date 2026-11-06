//! MOD-REG-01: site-level Book registration queue, not chapter moderation.
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
use tor_accounts::{
    repositories::{
        moderation_books::ModerationBooksError,
        moderation_queue_age::ModerationQueueAgeSummary,
        moderation_registration::RegistrationCandidate,
    },
    services::security::session_token_digest,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegistrationQuery {
    before: Option<i64>,
    q: Option<String>,
    level: Option<String>,
    wait: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegistrationDecisionForm {
    csrf_token: String,
    expected_revision: i64,
    action: String,
    reason: Option<String>,
    confirmed: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/moderation/books/registrations", get(queue))
        .route("/moderation/books/{id}/registration", post(decide))
        .route("/moderation/books/{id}/registration-history", get(history))
}

fn error_code(e: ModerationBooksError) -> StatusCode {
    match e {
        ModerationBooksError::Denied => StatusCode::FORBIDDEN,
        ModerationBooksError::Invalid => StatusCode::BAD_REQUEST,
        ModerationBooksError::Missing => StatusCode::NOT_FOUND,
        ModerationBooksError::Stale => StatusCode::CONFLICT,
        ModerationBooksError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

struct RegistrationQueueFilters<'a> {
    search: Option<&'a str>,
    level: Option<&'a str>,
    wait: Option<&'a str>,
}

fn render_age_summary(summary: &ModerationQueueAgeSummary) -> String {
    let registrations_24 = summary.registrations.at_least_24h;
    let registrations_72 = summary.registrations.at_least_72h;
    let covers_24 = summary.covers.at_least_24h;
    let covers_72 = summary.covers.at_least_72h;
    let deletions_24 = summary.deletions.at_least_24h;
    let deletions_72 = summary.deletions.at_least_72h;
    format!(
        r#"<section aria-labelledby="moderation-wait-heading">
        <h2 id="moderation-wait-heading">Контроль времени ожидания</h2>
        <p>Служебная сводка текущих заявок. Пороги 24 и 72 часа —
           ориентиры для ручной проверки, а не утверждённые SLA
           и не автоматические уведомления.</p>
        <ul>
          <li>Регистрация:
            <a href="/moderation/books/registrations?wait=gte24">от 24 ч: {registrations_24}</a>,
            <a href="/moderation/books/registrations?wait=gte72">от 72 ч: {registrations_72}</a>
            (время от создания книги, только первичная заявка)</li>
          <li>Обложки:
            <a href="/moderation/covers?wait=gte24">от 24 ч: {covers_24}</a>,
            <a href="/moderation/covers?wait=gte72">от 72 ч: {covers_72}</a></li>
          <li>Удаление:
            <a href="/moderation/books/deletions?wait=gte24">от 24 ч: {deletions_24}</a>,
            <a href="/moderation/books/deletions?wait=gte72">от 72 ч: {deletions_72}</a>
            (без заявок на собственные книги)</li>
        </ul></section>"#
    )
}

fn render(
    items: &[RegistrationCandidate],
    count: i64,
    csrf: &str,
    covers_count: i64,
    deletions_count: i64,
    filters: RegistrationQueueFilters<'_>,
    age: &ModerationQueueAgeSummary,
) -> String {
    let age_summary = render_age_summary(age);
    let token = pages::escape_html(csrf);
    let search = pages::escape_html(filters.search.unwrap_or(""));
    let level = filters.level.unwrap_or("");
    let selected = |tier: &str| if level == tier { " selected" } else { "" };
    let all_selected = selected("");
    let basic_selected = selected("basic");
    let adult_selected = selected("adult");
    let pro_selected = selected("pro_adult");
    let legacy_selected = selected("legacy");
    let wait = filters.wait.unwrap_or("");
    let wait_selected = |bucket: &str| if wait == bucket { " selected" } else { "" };
    let wait_all = wait_selected("");
    let wait_recent = wait_selected("lt24");
    let wait_day = wait_selected("gte24");
    let wait_three = wait_selected("gte72");
    let mut cards = String::new();
    for item in items {
        let title = pages::escape_html(&item.title);
        let owner = item
            .owner_user_id
            .map_or_else(|| "—".to_owned(), |id| id.to_string());
        cards.push_str(&format!(
            r#"<article class="me-now-card">
            <h2>{title}</h2>
            <p>Книга #{book} · Владелец #{owner} · Ревизия {rev}</p>
            <p><a href="/moderation/books/{book}/history">Все решения модерации</a></p>
            <details class="moderation-review-confirm" open>
            <summary>Проверить заявку на регистрацию книги #{book}</summary>
            <p>Проверьте название и ревизию. Одобрение не публикует авторский черновик.</p>
            <form method="post" action="/moderation/books/{book}/registration">
              <input type="hidden" name="csrf_token" value="{token}">
              <input type="hidden" name="expected_revision" value="{rev}">
              <label for="registration-reason-{book}">Причина отказа (обязательна)</label>
              <textarea id="registration-reason-{book}" name="reason"
                        maxlength="2000" rows="2"></textarea>
              <button type="submit" name="action" value="approve">Продолжить одобрение</button>
              <button type="submit" name="action" value="reject">Продолжить отказ</button>
            </form></details></article>"#,
            book = item.book_id,
            rev = item.revision,
        ));
    }
    if cards.is_empty() {
        cards.push_str("<p>Нет книг, ожидающих регистрацию.</p>");
    }
    if items.len() == 40 {
        if let Some(last) = items.last() {
            cards.push_str(&format!(
                r#"<form method="get" action="/moderation/books/registrations">
                <input type="hidden" name="before" value="{}">
                <input type="hidden" name="q" value="{search}">
                <input type="hidden" name="level" value="{level}">
                <input type="hidden" name="wait" value="{wait}">
                <button type="submit">Следующие заявки</button></form>"#,
                last.book_id
            ));
        }
    }
    pages::layout(
        "Модерация регистрации книг",
        "Закрытая очередь утверждения регистрации.",
        &format!(
            r#"<section class="me-page"><div class="container">
            <h1>Кабинет модератора</h1>
            <nav aria-label="Очереди модератора">
              <a href="/moderation/books/registrations" aria-current="page">Регистрация ({count})</a>
              <a href="/moderation/covers">Обложки ({covers_count})</a>
              <a href="/moderation/books/deletions">Удаление ({deletions_count})</a>
              <a href="/moderation/books/control">Контроль</a>
            </nav>
              <p><a href="/moderation/books/reviews">Журнал рассмотренных заявок</a></p>
            {age_summary}
            <form method="get" action="/moderation/books/registrations">
              <label for="registration-search">Поиск по названию или ID книги</label>
              <input id="registration-search" name="q" maxlength="120" value="{search}">
              <label for="registration-level">Возрастная категория книги</label>
              <select id="registration-level" name="level">
                <option value=""{all_selected}>Все уровни</option>
                <option value="basic"{basic_selected}>Без ограничений</option>
                <option value="adult"{adult_selected}>18+</option>
                <option value="pro_adult"{pro_selected}>21+</option>
                <option value="legacy"{legacy_selected}>Не указан</option>
              </select>
              <label for="registration-wait">Время ожидания регистрации</label>
              <select id="registration-wait" name="wait">
                <option value=""{wait_all}>Любое время</option>
                <option value="lt24"{wait_recent}>До 24 часов</option>
                <option value="gte24"{wait_day}>От 24 часов</option>
                <option value="gte72"{wait_three}>От 72 часов</option>
              </select>
              <button type="submit">Найти</button>
              <a href="/moderation/books/registrations">Сбросить</a>
            </form>
            <p>Одобрение регистрации не публикует авторский черновик,
               а новые главы не проходят отдельную модерацию.</p>
            {cards}
            <p><a href="/moderation/books/control">Контроль и удержания книг</a></p>
            </div></section>"#
        ),
    )
    .0
}

pub(super) async fn queue(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<RegistrationQuery>,
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
    let count = match state
        .accounts
        .moderation_registration_pending_count(&digest)
        .await
    {
        Ok(count) => count,
        Err(e) => return deny(error_code(e)),
    };
    let covers_count = match tor_books::repositories::cover_review::PgCoverReview::new(
        state.accounts.pool().clone(),
    )
    .pending_count(&digest)
    .await
    {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let deletions_count = match state
        .accounts
        .moderation_pending_deletion_count(&digest)
        .await
    {
        Ok(count) => count,
        Err(e) => return deny(error_code(e)),
    };
    let age_summary = match state.accounts.moderation_queue_age_summary(&digest).await {
        Ok(summary) => summary,
        Err(error) => return deny(error_code(error)),
    };
    let rows = match state
        .accounts
        .moderation_registration_queue_with_wait_age(
            &digest,
            query.before,
            query.q.as_deref(),
            query.level.as_deref().filter(|level| !level.is_empty()),
            query.wait.as_deref().filter(|wait| !wait.is_empty()),
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => return deny(error_code(e)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        Html(render(
            &rows,
            count,
            &csrf,
            covers_count,
            deletions_count,
            RegistrationQueueFilters {
                search: query.q.as_deref(),
                level: query.level.as_deref(),
                wait: query.wait.as_deref(),
            },
            &age_summary,
        )),
        &cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn decide(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<RegistrationDecisionForm>,
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
    let confirmed = match super::super::moderation_confirm::was_confirmed(form.confirmed.as_deref())
    {
        Ok(confirmed) => confirmed,
        Err(status) => return super::decision_error(status, "/moderation/books/registrations"),
    };
    if !confirmed {
        let reason = form.reason.as_deref().unwrap_or("").trim();
        if id <= 0
            || form.expected_revision <= 0
            || !matches!(form.action.as_str(), "approve" | "reject")
            || reason.len() > 2000
            || (form.action == "reject" && reason.is_empty())
        {
            return super::decision_error(
                StatusCode::BAD_REQUEST,
                "/moderation/books/registrations",
            );
        }
        return super::super::moderation_confirm::confirmation_page(
            super::super::moderation_confirm::ReviewIntent {
                kind: "registration",
                subject_id: id,
                revision: form.expected_revision,
                action: &form.action,
                reason,
                csrf: &form.csrf_token,
            },
        );
    }
    match state
        .accounts
        .moderate_book_registration(
            &digest,
            id,
            form.expected_revision,
            &form.action,
            form.reason.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/books/registrations"), &[]),
        Err(e) => super::decision_error(error_code(e), "/moderation/books/registrations"),
    }
}

pub(super) async fn history(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
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
    let approval = match state
        .accounts
        .private_book_registration_approver(&digest, id)
        .await
    {
        Ok(item) => item,
        Err(e) => return deny(error_code(e)),
    };
    let info = match approval {
        Some(item) => format!(
            "<p>Первоначально одобрил модератор: {} (#{}) · время Unix {}</p>",
            pages::escape_html(&item.reviewer_name),
            item.reviewer_user_id,
            item.approved_unix
        ),
        None => "<p>Подтверждённое первоначальное одобрение отсутствует.</p>".to_owned(),
    };
    private_response(
        pages::layout(
            "История регистрации книги",
            "Только модераторы сайта и администраторы.",
            &format!(
                "<section class=\"me-page\"><div class=\"container\">
                 <h1>Системная история регистрации книги #{id}</h1>{info}
                 <a href=\"/moderation/books/registrations\">Очередь регистрации</a>
                 </div></section>"
            ),
        ),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_hides_private_metadata_from_public_book() {
        let rows = vec![RegistrationCandidate {
            book_id: 51,
            title: "<script>private</script>".into(),
            owner_user_id: Some(37),
            revision: 1,
        }];
        let body = render(
            &rows,
            1,
            "csrf",
            2,
            3,
            RegistrationQueueFilters {
                search: None,
                level: None,
                wait: None,
            },
            &ModerationQueueAgeSummary::default(),
        );
        assert!(body.contains("&lt;script&gt;private&lt;/script&gt;"));
        assert!(!body.contains("<script>"));
        assert!(body.contains("Регистрация (1)"));
        assert!(body.contains("moderation-review-confirm"));
        assert!(body.contains("name=\"q\""));
        assert!(body.contains("name=\"level\""));
        assert!(body.contains("name=\"wait\""));
        assert!(body.contains("Контроль времени ожидания"));
        assert!(body.contains("/moderation/covers?wait=gte72"));
        assert!(body.contains("/moderation/books/deletions?wait=gte24"));
        assert!(body.contains("Обложки (2)"));
        assert!(body.contains("Удаление (3)"));
        assert!(body.contains("/moderation/books/51/registration"));
        assert!(!body.contains("reviewer_user_id"));
        let response = private_response(Html(body), &[]);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8",
            "moderator queue must render functional forms in Chromium"
        );
    }
}
