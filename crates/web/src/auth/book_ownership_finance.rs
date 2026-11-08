//! S030: Book ownership and creator-pool revision settings. Never a Team ACL
//! or a payment instruction; transfer and share replacement are separate acts.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Write};
use tor_accounts::{
    repositories::{
        book_step_up::{BOOK_SHARES_SCOPE, BOOK_TRANSFER_SCOPE},
        step_up::StepUpIssueError,
    },
    services::security::{session_token_digest, verify_password_for_step_up, VerifiedPassword},
};
use tor_books::{
    domain::shares::{RevenueShareError, ShareAssignment},
    repositories::{
        owner_transfer::{owner_transfer_payload_digest, BookOwnershipSettings, PgOwnerTransfer},
        revenue_shares::{revenue_share_payload_digest, PgRevenueShares},
    },
    services::authoring::AuthorError,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferForm {
    csrf_token: Option<String>,
    expected_owner_id: i64,
    target_display_name: String,
    account_password: String,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/books/{id}/settings/ownership", get(page))
        .route("/me/books/{id}/settings/ownership/transfer", post(transfer))
        .route(
            "/me/books/{id}/settings/ownership/shares",
            post(save_shares),
        )
        .layer(DefaultBodyLimit::max(8192))
}

fn render(id: i64, view: &BookOwnershipSettings, csrf: &str) -> Html<String> {
    let title = pages::escape_html(&view.title);
    let csrf = pages::escape_html(csrf);
    let tier = match view.access_level.as_deref() {
        Some("basic") => "Без ограничений",
        Some("adult") => "18+",
        Some("pro_adult") => "21+",
        _ => "Не подтверждён",
    };
    let mut rows = String::new();
    for member in &view.contributors {
        write!(
            rows,
            "<label for=\"share_{}\">{} (#{}, {}) — доля, %</label>\
             <input id=\"share_{}\" name=\"share_{}\" type=\"number\" min=\"0\" max=\"100\"\
               step=\"1\" value=\"{}\" required>",
            member.user_id,
            pages::escape_html(&member.display_name),
            member.user_id,
            pages::escape_html(&member.role),
            member.user_id,
            member.user_id,
            member.share_percent,
        )
        .expect("write HTML");
    }
    let shares = if let Some(revision) = view.share_revision {
        format!(
            r#"<h2>Доли авторского дохода</h2>
            <p>Изменение применяется только к будущим продажам. Старые покупки
            сохраняют первоначальную редакцию долей. Сумма положительных долей
            должна составлять ровно 100%.</p>
            <form class="auth-form" method="post"
                  action="/me/books/{id}/settings/ownership/shares">
              <input type="hidden" name="csrf_token" value="{csrf}">
              <input type="hidden" name="expected_revision" value="{revision}">
              {rows}
              <label for="share-reason">Причина изменения</label>
              <input id="share-reason" name="reason" maxlength="160" required>
              <label for="share-password">Подтвердите пароль аккаунта</label>
              <input id="share-password" name="account_password" type="password"
                     autocomplete="current-password" required>
              <button class="button" type="submit">Сохранить новую редакцию долей</button>
            </form>"#
        )
    } else {
        String::from("<p>Историческое распределение не подтверждено. Изменение долей закрыто.</p>")
    };
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card"><h1>Владелец и доли — {title}</h1>
        <p>Возрастной уровень: {tier}. Владелец аккаунта: #{owner}.</p>
        <h2>Передать книгу</h2>
        <p>Передача не изменяет команду, доли и уже приобретённые редакции.
        Получатель должен иметь подходящий возрастной допуск. Книга с любой
        историей участия в Layer пока не может быть передана: будущие продления
        могут восстанавливать права из прежних составов Layer. Для книги,
        привязанной к команде, получатель должен состоять в этой команде.</p>
        <form class="auth-form" method="post"
              action="/me/books/{id}/settings/ownership/transfer">
          <input type="hidden" name="csrf_token" value="{csrf}">
          <input type="hidden" name="expected_owner_id" value="{owner}">
          <label for="target-display-name">Точный ник нового владельца</label>
          <input id="target-display-name" name="target_display_name" maxlength="80"
                 autocomplete="off" required>
          <label for="transfer-password">Подтвердите пароль аккаунта</label>
          <input id="transfer-password" name="account_password" type="password"
                 autocomplete="current-password" required>
          <button class="button" type="submit">Передать книгу</button>
        </form>
        {shares}
        <p><a href="/me/books/{id}/settings">Все настройки книги</a></p>
        </div></div></section>"#,
        owner = view.owner_id,
    );
    pages::layout(
        "Владелец и доли доходов",
        "Приватные настройки книги.",
        &content,
    )
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    session_user(state, headers)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn transfer_error(error: AuthorError) -> StatusCode {
    match error {
        AuthorError::Invalid => StatusCode::BAD_REQUEST,
        AuthorError::Conflict => StatusCode::CONFLICT,
        AuthorError::Missing | AuthorError::Forbidden => StatusCode::NOT_FOUND,
        AuthorError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn share_error(error: RevenueShareError) -> StatusCode {
    match error {
        RevenueShareError::Invalid => StatusCode::BAD_REQUEST,
        RevenueShareError::Conflict => StatusCode::CONFLICT,
        RevenueShareError::Forbidden
        | RevenueShareError::Missing
        | RevenueShareError::InactiveRecipient => StatusCode::NOT_FOUND,
        RevenueShareError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn issue_error(error: StepUpIssueError) -> StatusCode {
    match error {
        StepUpIssueError::InvalidIntent => StatusCode::BAD_REQUEST,
        StepUpIssueError::Forbidden => StatusCode::FORBIDDEN,
        StepUpIssueError::StepUpRequired => StatusCode::UNAUTHORIZED,
        StepUpIssueError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn fresh_password(
    state: &AuthState,
    actor: i64,
    password: String,
) -> Result<VerifiedPassword, StatusCode> {
    if password.is_empty() || password.len() > 512 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let key = session_token_digest(&format!("auth:s030-sensitive:{actor}"));
    match state.accounts.permit_auth_attempt(&key, 5, 600).await {
        Ok(true) => {}
        Ok(false) => return Err(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    }
    let hash = state
        .accounts
        .password_hash_for_user(actor)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    state
        .password_jobs
        .run(move || verify_password_for_step_up(&password, &hash))
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)
}

async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(StatusCode::UNAUTHORIZED) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let settings = match PgOwnerTransfer::new(state.accounts.pool().clone())
        .settings(actor, id)
        .await
    {
        Ok(Some(view)) => view,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(transfer_error(error)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(id, &settings, &csrf), &cookies)
}

async fn transfer(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<TransferForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if id <= 0 || form.expected_owner_id != actor {
        return deny(StatusCode::BAD_REQUEST);
    }
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&session);
    let proof = match fresh_password(&state, actor, form.account_password).await {
        Ok(proof) => proof,
        Err(status) => return deny(status),
    };
    let payload =
        owner_transfer_payload_digest(actor, id, form.expected_owner_id, &form.target_display_name);
    let token = match state
        .accounts
        .issue_book_mutation_step_up(
            actor,
            &digest,
            &proof,
            id,
            form.expected_owner_id,
            BOOK_TRANSFER_SCOPE,
            &payload,
        )
        .await
    {
        Ok(token) => token,
        Err(error) => return deny(issue_error(error)),
    };
    match PgOwnerTransfer::new(state.accounts.pool().clone())
        .transfer(
            actor,
            id,
            form.expected_owner_id,
            &form.target_display_name,
            &digest,
            &token,
        )
        .await
    {
        Ok(()) => {
            let key = session_token_digest(&format!("auth:s030-sensitive:{actor}"));
            let _ = state.accounts.clear_auth_attempt(&key).await;
            private_response(Redirect::to("/me/books"), &[])
        }
        Err(error) => deny(transfer_error(error)),
    }
}

async fn save_shares(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(
            &headers,
            form.get("csrf_token").map(String::as_str).unwrap_or(""),
        )
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if id <= 0 {
        return deny(StatusCode::BAD_REQUEST);
    }
    let Some(session) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let expected_revision = match form
        .get("expected_revision")
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(revision) if revision > 0 => revision,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let Some(reason) = form.get("reason").filter(|v| !v.is_empty()) else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let mut assignments = Vec::new();
    for (key, value) in &form {
        let Some(user) = key.strip_prefix("share_") else {
            if matches!(
                key.as_str(),
                "csrf_token" | "expected_revision" | "reason" | "account_password"
            ) {
                continue;
            }
            return deny(StatusCode::BAD_REQUEST);
        };
        let (Ok(user_id), Ok(percent)) = (user.parse::<i64>(), value.parse::<i16>()) else {
            return deny(StatusCode::BAD_REQUEST);
        };
        if user_id <= 0 || !(0..=100).contains(&percent) {
            return deny(StatusCode::BAD_REQUEST);
        }
        if percent > 0 {
            assignments.push(ShareAssignment {
                user_id,
                share_percent: percent,
            });
        }
    }
    let Some(password) = form.get("account_password") else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let digest = session_token_digest(&session);
    let proof = match fresh_password(&state, actor, password.clone()).await {
        Ok(proof) => proof,
        Err(status) => return deny(status),
    };
    let payload =
        match revenue_share_payload_digest(actor, id, expected_revision, &assignments, reason) {
            Ok(payload) => payload,
            Err(error) => return deny(share_error(error)),
        };
    let token = match state
        .accounts
        .issue_book_mutation_step_up(
            actor,
            &digest,
            &proof,
            id,
            expected_revision,
            BOOK_SHARES_SCOPE,
            &payload,
        )
        .await
    {
        Ok(token) => token,
        Err(error) => return deny(issue_error(error)),
    };
    match PgRevenueShares::new(state.accounts.pool().clone())
        .replace_schedule_verified(
            actor,
            id,
            expected_revision,
            &assignments,
            reason,
            &digest,
            &token,
        )
        .await
    {
        Ok(_) => {
            let key = session_token_digest(&format!("auth:s030-sensitive:{actor}"));
            let _ = state.accounts.clear_auth_attempt(&key).await;
            private_response(
                Redirect::to(&format!("/me/books/{id}/settings/ownership")),
                &[],
            )
        }
        Err(error) => deny(share_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s030_owner_page_escapes_nicks_and_preserves_separate_actions() {
        let view = BookOwnershipSettings {
            title: "<script>attack</script>".into(),
            access_level: Some("adult".into()),
            owner_id: 5,
            share_revision: Some(3),
            contributors: vec![
                tor_books::repositories::owner_transfer::BookContributor {
                    user_id: 5,
                    display_name: "<img src=x>".into(),
                    role: "owner".into(),
                    share_percent: 70,
                },
                tor_books::repositories::owner_transfer::BookContributor {
                    user_id: 6,
                    display_name: "Translator".into(),
                    role: "translator".into(),
                    share_percent: 30,
                },
            ],
        };
        let html = render(71, &view, "CSRF").0;
        assert!(!html.contains("<script>attack"));
        assert!(!html.contains("<img src=x>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("share_5"));
        assert!(html.contains("value=\"70\""));
        assert!(html.contains("expected_revision\" value=\"3\""));
        assert!(html.contains("expected_owner_id\" value=\"5\""));
        assert!(html.contains("/settings/ownership/transfer"));
        assert!(html.contains("/settings/ownership/shares"));
        assert!(!html.contains("name=\"price\""));
    }
}
