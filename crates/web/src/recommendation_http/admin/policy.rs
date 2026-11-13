//! One protected, no-JS control surface for the global ORGANIC shelf policy.
//! No viewer events, cookies in SQL, advertising billing or rollout writes.
use super::{actor, csrf, pages, Runtime};
use crate::{auth_cookies::deny, pages as html};
use axum::{
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::{fmt::Write, time::Duration};
use tor_accounts::services::security::session_token_digest;
use tor_recommendations::{
    domain::feedback::{Feed, FeedbackError},
    services::reader_end_shelf::ShelfSharePolicy,
};

const PATH: &str = "/admin/recommendations";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    book: Option<i64>,
    chapter: Option<i64>,
    changed: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PolicyForm {
    action: Option<String>,
    expected_revision: Option<String>,
    author_pct: Option<String>,
    teammate_pct: Option<String>,
    other_authors_pct: Option<String>,
    no_team_author_pct: Option<String>,
    no_team_other_authors_pct: Option<String>,
    target_revision: Option<String>,
    csrf_token: Option<String>,
}

fn canonical_revision(value: Option<&str>) -> Result<i64, StatusCode> {
    let value = value.ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    value
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0 && v.to_string() == value)
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)
}

fn percent(value: Option<&str>) -> Result<i32, StatusCode> {
    let value = value.ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    value
        .parse::<i32>()
        .ok()
        .filter(|v| (0..=100).contains(v))
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)
}

impl PolicyForm {
    fn submitted_policy(&self) -> Result<ShelfSharePolicy, StatusCode> {
        let ordinary = [
            percent(self.author_pct.as_deref())?,
            percent(self.teammate_pct.as_deref())?,
            percent(self.other_authors_pct.as_deref())?,
        ];
        let without_team = [
            percent(self.no_team_author_pct.as_deref())?,
            percent(self.no_team_other_authors_pct.as_deref())?,
        ];
        ShelfSharePolicy::validated(ordinary, without_team)
            .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)
    }
}

fn error_status(error: FeedbackError) -> StatusCode {
    match error {
        FeedbackError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        FeedbackError::Forbidden => StatusCode::FORBIDDEN,
        FeedbackError::NotFound => StatusCode::NOT_FOUND,
        FeedbackError::Conflict | FeedbackError::IdempotencyConflict => StatusCode::CONFLICT,
        FeedbackError::Unavailable | FeedbackError::Limited => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn hidden(name: &str, value: &str) -> String {
    format!(
        r#"<input type="hidden" name="{name}" value="{}">"#,
        html::escape_html(value)
    )
}

fn security(csrf_token: &str, revision: i64) -> String {
    format!(
        "{}{}",
        hidden("csrf_token", csrf_token),
        hidden("expected_revision", &revision.to_string())
    )
}

fn render(
    snapshot: &tor_recommendations::repositories::ShelfShareSnapshot,
    history: &[tor_recommendations::repositories::ShelfShareAuditRow],
    csrf_token: &str,
    enabled: bool,
    changed: Option<i64>,
    preview: Option<(i64, i64, Feed)>,
) -> String {
    let [author, team, other] = snapshot.policy.with_team();
    let [alone_author, alone_other] = snapshot.policy.without_team();
    let mut content = String::from(
        r#"<section class="section"><div class="container">
        <nav><a href="/me">Кабинет</a> · <a href="/admin/audience">Аудитория</a></nav>
        <h1>Управление рекомендациями</h1>
        <p>Единая панель органики после бесплатных глав. Доли описывают
        распределение по множеству выдач, а не количество мест внутри одной полки.</p>"#,
    );
    let _ = write!(
        content,
        "<p data-testid=\"rec-policy-revision\">Ревизия политики: {}. Публичная выдача: {}.</p>",
        snapshot.revision,
        if enabled { "включена" } else { "отключена флагом TOR_RECOMMENDATIONS_ENABLED" }
    );
    if let Some(changed) = changed.filter(|value| *value == snapshot.revision) {
        let _ = write!(
            content,
            "<p role=\"status\">Политика сохранена. Текущая ревизия — {changed}.</p>"
        );
    }
    content.push_str(
        r#"<section class="book-section-card"><h2>Доли органических рекомендаций</h2>
        <p>Автор — владелец исходной книги. «Другие авторы» — владельцы вне команды.
        Редакционные назначения не входят в эти проценты.</p>"#,
    );
    let _ = write!(
        content,
        "<form class=\"auth-form author-form\" method=\"post\" action=\"{PATH}\" data-testid=\"rec-policy-form\">{}{}",
        security(csrf_token, snapshot.revision),
        hidden("action", "save")
    );
    content.push_str("<h3>Для книги с командой</h3>");
    for (name, label, value) in [
        ("author_pct", "Автор", author),
        ("teammate_pct", "Команда", team),
        ("other_authors_pct", "Другие авторы", other),
    ] {
        let _ = write!(
            content,
            "<label>{label}, %<input type=\"number\" name=\"{name}\" min=\"0\" max=\"100\" step=\"1\" value=\"{value}\" required></label>"
        );
    }
    content.push_str("<h3>Для книги без команды</h3>");
    for (name, label, value) in [
        ("no_team_author_pct", "Автор", alone_author),
        ("no_team_other_authors_pct", "Другие авторы", alone_other),
    ] {
        let _ = write!(
            content,
            "<label>{label}, %<input type=\"number\" name=\"{name}\" min=\"0\" max=\"100\" step=\"1\" value=\"{value}\" required></label>"
        );
    }
    content.push_str(
        "<p>Обе суммы должны быть ровно 100%. Нулевое значение запрещает категорию.</p>\
         <button class=\"button\" type=\"submit\">Сохранить доли</button></form>",
    );
    let _ = write!(
        content,
        "<form method=\"post\" action=\"{PATH}\" class=\"author-form\">{}{}<button class=\"button\" type=\"submit\">Вернуть 50/40/10 и 70/30</button></form></section>",
        security(csrf_token, snapshot.revision),
        hidden("action", "defaults")
    );
    content.push_str(
        r#"<section class="book-section-card"><h2>Предпросмотр полки</h2>
        <p>Гостевой basic-доступ. Здесь не создаются показы, клики или персональная история.
        Фактическая полка может содержать меньше пяти книг, если допустимых кандидатов недостаточно.</p>
        <form method="get" action="/admin/recommendations" class="author-form">
        <label>ID исходной книги<input type="number" name="book" min="1" required></label>
        <label>ID бесплатной главы<input type="number" name="chapter" min="1" required></label>
        <button class="button" type="submit">Посмотреть состав</button></form>"#,
    );
    if let Some((book, chapter, feed)) = preview {
        let _ = write!(
            content,
            "<p data-testid=\"rec-preview-counts\">Исходная книга {book}; глава {chapter}; подходящих карточек: {} из 5.</p><ol>",
            feed.items.len()
        );
        for item in feed.items {
            let Ok(id) = item.book_id.parse::<i64>() else {
                continue;
            };
            if id <= 0 {
                continue;
            }
            let source = match item.source.as_str() {
                "author" => "Автор",
                "teammate" => "Команда",
                "site" | "other_authors" => "Другие авторы",
                _ => "Неизвестный источник",
            };
            let _ = write!(
                content,
                "<li><a href=\"/books/{id}\">{}</a> — {source}</li>",
                html::escape_html(&item.title)
            );
        }
        content.push_str("</ol>");
    }
    content.push_str(
        r#"</section><section class="book-section-card"><h2>Управление и измерения</h2>
        <p><a href="/admin/promotion/editorial">Редакционные назначения</a> ·
        <a href="/me/recommendations/contextual">Контекстная модель</a> ·
        <a href="/admin/audience/exposure">Показы и переходы</a> ·
        <a href="/admin/audience/reading">Подтверждённое чтение</a></p>
        <p>Редакционные места не изменяют органические доли; просмотры карточек,
        переходы и чтение — разные показатели.</p></section>
        <section class="book-section-card"><h2>История и откат</h2>
        <p>Возврат создаёт новую ревизию. Старые записи аудита не изменяются.</p>"#,
    );
    for row in history {
        let actor = row
            .actor_user_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "система".to_owned());
        let [a, t, o] = row.with_team;
        let [na, no] = row.without_team;
        let _ = write!(
            content,
            "<article><p>Ревизия {} · {} UTC · администратор {} · {}<br>С командой: {a}/{t}/{o}; без команды: {na}/{no}</p>",
            row.revision,
            html::escape_html(&row.changed_at),
            actor,
            html::escape_html(&row.action)
        );
        if row.revision != snapshot.revision {
            let _ = write!(
                content,
                "<form method=\"post\" action=\"{PATH}\" class=\"author-form\">{}{}{}<button type=\"submit\" class=\"button\">Вернуть ревизию {}</button></form>",
                security(csrf_token, snapshot.revision),
                hidden("action", "restore"),
                hidden("target_revision", &row.revision.to_string()),
                row.revision
            );
        }
        content.push_str("</article>");
    }
    content.push_str("</section></div></section>");
    content
}

pub(crate) async fn page(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> Response {
    let operation = async {
        actor(&state, &headers).await?;
        let snapshot = state
            .store
            .shelf_share_policy()
            .await
            .map_err(error_status)?;
        let history = state.store.shelf_share_audit().await.map_err(error_status)?;
        let preview = match (query.book, query.chapter) {
            (None, None) => None,
            (Some(book), Some(chapter)) if book > 0 && chapter > 0 => {
                let feed = state
                    .store
                    .reader_end_shelf(None, book, chapter)
                    .await
                    .map_err(error_status)?;
                Some((book, chapter, feed))
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        };
        actor(&state, &headers).await?;
        let (csrf_token, cookies) = state.cookies.csrf_for_page(&headers);
        let content = render(
            &snapshot,
            &history,
            &csrf_token,
            state.enabled,
            query.changed,
            preview,
        );
        Ok::<Response, StatusCode>(pages::secure_page(
            html::layout("Управление рекомендациями", "Политика органических полок", &content),
            &cookies,
        ))
    };
    match tokio::time::timeout(Duration::from_millis(1800), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => pages::secure_page(deny(error), &[]),
        Err(_) => pages::secure_page(deny(StatusCode::SERVICE_UNAVAILABLE), &[]),
    }
}

pub(crate) async fn save(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<PolicyForm>,
) -> Response {
    let operation = async {
        actor(&state, &headers).await?;
        csrf(
            &state,
            &headers,
            body.csrf_token.as_deref().unwrap_or_default(),
        )?;
        let expected = canonical_revision(body.expected_revision.as_deref())?;
        let policy = match body.action.as_deref() {
            Some("save") => body.submitted_policy()?,
            Some("defaults") => ShelfSharePolicy::default(),
            Some("restore") => {
                let target = canonical_revision(body.target_revision.as_deref())?;
                state
                    .store
                    .shelf_share_audit_policy(target)
                    .await
                    .map_err(error_status)?
                    .ok_or(StatusCode::NOT_FOUND)?
            }
            _ => return Err(StatusCode::UNPROCESSABLE_ENTITY),
        };
        let token = state
            .cookies
            .session_token(&headers)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let digest = session_token_digest(&token);
        let revision = state
            .store
            .set_shelf_share_policy(&digest, expected, policy)
            .await
            .map_err(error_status)?;
        Ok::<Response, StatusCode>(pages::secure_page(
            Redirect::to(&format!("{PATH}?changed={revision}")),
            &[],
        ))
    };
    match tokio::time::timeout(Duration::from_millis(950), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => pages::secure_page(deny(error), &[]),
        Err(_) => pages::secure_page(deny(StatusCode::SERVICE_UNAVAILABLE), &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_require_two_correct_sums_even_when_zero_is_explicit() {
        let mut form = PolicyForm {
            action: Some("save".into()),
            expected_revision: Some("1".into()),
            author_pct: Some("40".into()),
            teammate_pct: Some("30".into()),
            other_authors_pct: Some("30".into()),
            no_team_author_pct: Some("60".into()),
            no_team_other_authors_pct: Some("40".into()),
            target_revision: None,
            csrf_token: None,
        };
        assert_eq!(
            form.submitted_policy().unwrap().with_team(),
            [40, 30, 30]
        );
        form.other_authors_pct = Some("20".into());
        assert!(form.submitted_policy().is_err());
        form.other_authors_pct = Some("0".into());
        form.author_pct = Some("70".into());
        assert!(form.submitted_policy().is_ok());
        form.no_team_other_authors_pct = Some("-1".into());
        assert!(form.submitted_policy().is_err());
        form.no_team_other_authors_pct = Some("not a number".into());
        assert!(form.submitted_policy().is_err());
    }

    #[test]
    fn cas_revision_rejects_invalid_and_noncanonical_values() {
        for value in [None, Some(""), Some("0"), Some("01"), Some("-1"), Some("1e3")] {
            assert!(canonical_revision(value).is_err());
        }
        assert_eq!(canonical_revision(Some("42")), Ok(42));
    }
}
