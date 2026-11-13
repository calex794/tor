//! Retired analytics controls still require the original live administrator and CSRF gates.
pub(super) mod contextual;
pub(super) mod shelf;
use super::{api, pages, Runtime};
use crate::{auth_cookies::deny, pages as html};
use axum::{
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_recommendations::{domain::feedback::FeedbackError, services::feedback::validate_revision};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QualityQuery {
    days: Option<i32>,
    channel: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Refresh,
    Activate,
    Hold,
    Rollback,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    action: Action,
    generation: Option<String>,
    expected_revision: i64,
    percent: Option<i16>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlForm {
    action: Action,
    generation: Option<String>,
    expected_revision: i64,
    percent: Option<i16>,
    csrf_token: String,
}
async fn actor(state: &Runtime, headers: &HeaderMap) -> Result<i64, StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let verified = state
        .accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !verified.administrator {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(verified.actor.user_id.get())
}
fn csrf(state: &Runtime, headers: &HeaderMap, token: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, token) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
pub(super) async fn quality(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Query(query): Query<QualityQuery>,
) -> Response {
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    let max_days = match query.channel.as_deref() {
        None | Some("recommendation") => 30,
        Some("organic") => 180,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    if !(1..=max_days).contains(&query.days.unwrap_or(7)) {
        return api::error(FeedbackError::Invalid);
    }
    api::retired()
}

fn validate_control(body: &Control) -> Result<(), FeedbackError> {
    validate_revision(body.expected_revision)?;
    if body
        .percent
        .is_some_and(|value| ![0, 1, 5, 10, 25, 50, 100].contains(&value))
    {
        return Err(FeedbackError::Invalid);
    }
    if let Some(generation) = &body.generation {
        if generation.len() != 36
            || !generation.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        {
            return Err(FeedbackError::Invalid);
        }
    }
    if matches!(&body.action, Action::Activate) && body.generation.is_none() {
        return Err(FeedbackError::Invalid);
    }
    Ok(())
}

pub(super) async fn control(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Json(body): Json<Control>,
) -> Response {
    if let Err(status) = csrf(
        &state,
        &headers,
        headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    ) {
        return deny(status);
    }
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    if let Err(error) = validate_control(&body) {
        return api::error(error);
    }
    api::retired()
}

pub(super) async fn form_control(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<ControlForm>,
) -> Response {
    if let Err(status) = csrf(&state, &headers, &body.csrf_token) {
        return deny(status);
    }
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    if let Err(error) = validate_control(&Control {
        action: body.action,
        generation: body.generation,
        expected_revision: body.expected_revision,
        percent: body.percent,
    }) {
        return api::error(error);
    }
    api::retired()
}

pub(super) async fn page(State(state): State<Runtime>, headers: HeaderMap) -> Response {
    if let Err(status) = actor(&state, &headers).await {
        return deny(status);
    }
    // Keep ordinary explicit book settings available under a fresh live role.
    let role = match state
        .accounts
        .session_actor(&session_token_digest(
            &state.cookies.session_token(&headers).unwrap_or_default(),
        ))
        .await
    {
        Ok(Some(role)) if role.administrator => role,
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let books = match state.publishing.manageable_books(role, 0).await {
        Ok(value) => value,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut body = String::from(
        "<section class=\"section\"><div class=\"container\"><h1>Настройки рекомендаций</h1>\
         <p data-testid=\"recommendation-model-retired\">Прежний подбор по истории чтения и сбор событий прекращены. \
         Сейчас показываются новые доступные книги с учётом ваших явных скрытий.</p>\
         <p><a href=\"/admin/audience\">Исторические отчёты аудитории</a></p>\
         <h2>Явные настройки книг</h2><p>Категории и ручные связи книг остаются обычными настройками. \
         Новинки каталога остаются резервной выдачей. Контекстная модель жанров, тегов и ручных связей включается отдельно.</p><ul>",
    );
    for (id, title) in books {
        let _ = write!(
            body,
            "<li><a href=\"/me/books/{id}/recommendations\">{}</a></li>",
            html::escape_html(&title),
        );
    }
    body.push_str("</ul><p><a data-testid=\"contextual-manage-link\" href=\"/me/recommendations/contextual\">\u{41a}\u{43e}\u{43d}\u{442}\u{435}\u{43a}\u{441}\u{442}\u{43d}\u{430}\u{44f} \u{43c}\u{43e}\u{434}\u{435}\u{43b}\u{44c} \u{440}\u{435}\u{43a}\u{43e}\u{43c}\u{435}\u{43d}\u{434}\u{430}\u{446}\u{438}\u{439}</a></p></div></section>");
    body.push_str("<p><a href=\"/admin/recommendations\">Единая административная панель рекомендаций и долей</a></p>");
    body.push_str("<p><a href=\"/me/recommendations/shelf-preview\">Предпросмотр полки после бесплатной главы</a></p>");
    pages::secure_page(
        html::layout("Настройки рекомендаций", "Явные настройки книг", &body),
        &[],
    )
}

// REC-ADMIN-01: the sole write surface for the versioned global shelf ratios.
// The underlying F-owned CAS function checks the administrator session again
// inside PostgreSQL; H must additionally validate the live A role and CSRF.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ShareAction {
    Save,
    Defaults,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ShareForm {
    csrf_token: String,
    expected_revision: i64,
    action: ShareAction,
    author_pct: i32,
    teammate_pct: i32,
    other_authors_pct: i32,
    no_team_author_pct: i32,
    no_team_other_authors_pct: i32,
}

impl ShareForm {
    fn shares(&self) -> Result<([i32; 3], [i32; 2]), FeedbackError> {
        let values = match self.action {
            ShareAction::Save => (
                [self.author_pct, self.teammate_pct, self.other_authors_pct],
                [self.no_team_author_pct, self.no_team_other_authors_pct],
            ),
            ShareAction::Defaults => ([50, 40, 10], [70, 30]),
        };
        tor_recommendations::services::reader_end_shelf::ShelfSharePolicy::validated(
            values.0, values.1,
        )
        .map_err(|_| FeedbackError::Invalid)?;
        Ok(values)
    }
}

fn share_status(error: FeedbackError) -> StatusCode {
    match error {
        FeedbackError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        FeedbackError::Forbidden => StatusCode::FORBIDDEN,
        FeedbackError::Conflict => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn share_markup(
    policy: tor_recommendations::repositories::feedback::ShelfShareSnapshot,
    csrf: &str,
    enabled: bool,
) -> String {
    let with_team = policy.policy.with_team();
    let without_team = policy.policy.without_team();
    let mut out = String::from(
        r#"<section class="section"><div class="container audience-books-width"><h1>Управление рекомендациями</h1><p>Одна панель для органических рекомендаций и редакционных назначений. Изменение процентов не включает публикацию рекомендаций и не изменяет права на книги.</p>"#,
    );
    if !enabled {
        out.push_str(
            r#"<p role="status" data-testid="rec-policy-off">Публичные рекомендации выключены. Доли можно подготовить без включения выдачи.</p>"#,
        );
    }
    let _ = write!(
        out,
        r#"<section class="book-section-card" data-testid="rec-share-policy" data-revision="{}"><h2>Доли органических рекомендаций</h2><p>Версия настройки: {}. Проценты рассчитаны на множество органических показов, а не на отдельную полку из пяти книг. Редакционные размещения занимают свои места отдельно.</p><form method="post" action="/admin/recommendations" class="auth-form author-form" data-testid="rec-shares-form"><input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="expected_revision" value="{}"><fieldset><legend>Когда есть доступные книги команды (сумма 100%)</legend><label>Другие книги владельца, %<input name="author_pct" type="number" min="0" max="100" value="{}" required></label><label>Книги команды, %<input name="teammate_pct" type="number" min="0" max="100" value="{}" required></label><label>Книги других авторов, %<input name="other_authors_pct" type="number" min="0" max="100" value="{}" required></label></fieldset><fieldset><legend>Когда нет доступных командных книг (сумма 100%)</legend><label>Другие книги владельца, %<input name="no_team_author_pct" type="number" min="0" max="100" value="{}" required></label><label>Книги других авторов, %<input name="no_team_other_authors_pct" type="number" min="0" max="100" value="{}" required></label></fieldset><p>Установленные 0% запрещают соответствующую категорию, даже когда другая категория пуста. Отсутствующие книги не заменяются фиктивными.</p><button class="button" type="submit" name="action" value="save">Сохранить доли</button><button class="button button--secondary" type="submit" name="action" value="defaults" formnovalidate>Восстановить 50/40/10 и 70/30</button></form></section>"#,
        policy.revision,
        policy.revision,
        html::escape_html(csrf),
        policy.revision,
        with_team[0],
        with_team[1],
        with_team[2],
        without_team[0],
        without_team[1]
    );
    out.push_str(
        r#"<section class="book-section-card"><h2>Проверить выдачу</h2><p>Предпросмотр бесплатной главы не создаёт подтверждений показа и не изменяет доли.</p><p><a href="/me/recommendations/shelf-preview">Предпросмотр органической полки</a></p></section><section class="book-section-card"><h2>Редакционные назначения и измерения</h2><p>Редакционный блок занимает не более четырёх из пяти мест, но не входит в проценты органики.</p><p><a href="/admin/promotion/editorial">Управление редакционными кампаниями</a> · <a href="/admin/audience/exposure">Показы и переходы</a></p><p><a href="/me/recommendations/contextual">Контекстная модель и версии</a> · <a href="/me/recommendations/manage">Ручные связи книг</a></p></section></div></section>"#,
    );
    out
}

pub(super) async fn shares_page(State(state): State<Runtime>, headers: HeaderMap) -> Response {
    let operation = async {
        actor(&state, &headers).await?;
        let policy = state
            .store
            .shelf_share_policy()
            .await
            .map_err(share_status)?;
        actor(&state, &headers).await?;
        let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
        Ok::<_, StatusCode>(pages::secure_page(
            html::layout(
                "Управление рекомендациями",
                "Доли рекомендаций и редакционные назначения",
                &share_markup(policy, &csrf, state.enabled),
            ),
            &cookies,
        ))
    };
    match tokio::time::timeout(std::time::Duration::from_millis(950), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(status)) => pages::secure_page(deny(status), &[]),
        Err(_) => pages::secure_page(deny(StatusCode::SERVICE_UNAVAILABLE), &[]),
    }
}

pub(super) async fn shares_submit(
    State(state): State<Runtime>,
    headers: HeaderMap,
    form: Result<Form<ShareForm>, axum::extract::rejection::FormRejection>,
) -> Response {
    let operation = async {
        actor(&state, &headers).await?;
        let Form(form) = form.map_err(|error| {
            let status = error.status();
            if status == StatusCode::BAD_REQUEST {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                status
            }
        })?;
        csrf(&state, &headers, &form.csrf_token)?;
        let (with_team, without_team) = form.shares().map_err(share_status)?;
        let token = state
            .cookies
            .session_token(&headers)
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let digest = session_token_digest(&token);
        state
            .store
            .set_shelf_share_policy(&digest, form.expected_revision, with_team, without_team)
            .await
            .map_err(share_status)?;
        Ok::<_, StatusCode>(axum::response::Redirect::to("/admin/recommendations").into_response())
    };
    let response = tokio::time::timeout(std::time::Duration::from_millis(950), operation)
        .await
        .unwrap_or(Err(StatusCode::SERVICE_UNAVAILABLE));
    pages::secure_page(
        match response {
            Ok(response) => response,
            Err(status) => deny(status),
        },
        &[],
    )
}

#[cfg(test)]
mod share_panel_tests {
    use super::*;
    use tor_recommendations::repositories::feedback::ShelfShareSnapshot;
    use tor_recommendations::services::reader_end_shelf::ShelfSharePolicy;

    #[test]
    fn admin_panel_renders_current_policy_without_client_side_code() {
        let snapshot = ShelfShareSnapshot {
            revision: 9,
            policy: ShelfSharePolicy::validated([40, 30, 30], [60, 40]).unwrap(),
        };
        let html = share_markup(snapshot, "csrf&<test>", false);
        assert!(html.contains(r#"data-revision="9""#));
        assert!(html.contains(r#"name="author_pct" type="number" min="0" max="100" value="40""#));
        assert!(html
            .contains(r#"name="no_team_author_pct" type="number" min="0" max="100" value="60""#));
        assert!(html.contains("csrf&amp;&lt;test&gt;"));
        assert!(html.contains("rec-policy-off"));
        assert!(html.contains("value=\"defaults\" formnovalidate"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("data-flow-proof"));
    }

    #[test]
    fn admin_form_rejects_invalid_shares_and_defaults_are_explicit() {
        let mut form = ShareForm {
            csrf_token: "server-only".into(),
            expected_revision: 1,
            action: ShareAction::Save,
            author_pct: 50,
            teammate_pct: 40,
            other_authors_pct: 20,
            no_team_author_pct: 70,
            no_team_other_authors_pct: 30,
        };
        assert_eq!(form.shares(), Err(FeedbackError::Invalid));
        form.other_authors_pct = 10;
        assert_eq!(form.shares(), Ok(([50, 40, 10], [70, 30])));
        form.action = ShareAction::Defaults;
        form.author_pct = -1;
        assert_eq!(form.shares(), Ok(([50, 40, 10], [70, 30])));
    }
}
