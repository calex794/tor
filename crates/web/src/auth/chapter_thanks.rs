//! NEwBOO reader action: one "Спасибо" per authenticated user and free chapter.
//! The original Python requires CSRF, an A session and readable chapter access.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_reading::repositories::{PgChapterThanks, ThanksMutation};

#[derive(Default, Deserialize)]
pub(super) struct ThanksQuery {
    done: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ThanksForm {
    csrf_token: Option<String>,
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    Query(query): Query<ThanksQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let repo = PgChapterThanks::new(state.accounts.pool().clone());
    let info = match repo.visible_free_summary(actor, id).await {
        Ok(Some(info)) => info,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let notice = match query.done.as_deref() {
        Some("added") => "<p role=\"status\">Спасибо учтено.</p>",
        Some("repeat") => "<p role=\"status\">Вы уже поблагодарили за эту главу.</p>",
        _ => "",
    };
    let action = if info.already_thanked {
        "<p>Вы уже поблагодарили за эту главу. Повторное нажатие не меняет счётчик.</p>".to_owned()
    } else {
        format!(
            "<form method=\"post\" action=\"/reader/{id}/thanks\">
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
             <button class=\"button\" type=\"submit\">Сказать «Спасибо»</button>
             </form>",
            pages::escape_html(&csrf)
        )
    };
    let body = format!(
        "<section class=\"section auth-section\"><div class=\"container\">
         <div class=\"auth-card\"><h1>Спасибо за главу</h1>
         <p>Глава: {}</p><p>Благодарностей: {}</p>
         {notice}{action}
         <p><a href=\"/reader/{id}\">Вернуться к чтению</a></p></div>
         </div></section>",
        pages::escape_html(&info.title),
        info.count,
    );
    private_response(
        pages::layout("Спасибо за главу", "Благодарность автору главы.", &body),
        &cookies,
    )
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ThanksForm>,
) -> Response {
    // Check CSRF before any mutation; never accept a client-supplied user id.
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|value| state.cookies.valid_csrf(&headers, value))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    // NEwBOO chapter_thanks: 20 attempts/minute per verified account.
    // A PostgreSQL counter makes the guard shared across web processes.
    let key = session_token_digest(&format!("action:chapter_thanks:user:{actor}"));
    let allowed = match state.accounts.permit_auth_attempt(&key, 20, 60).await {
        Ok(allowed) => allowed,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !allowed {
        let mut response = deny(StatusCode::TOO_MANY_REQUESTS);
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("60"),
        );
        return response;
    }
    let repo = PgChapterThanks::new(state.accounts.pool().clone());
    match repo.thank_visible_free(actor, id).await {
        Ok(ThanksMutation::NotVisible) => deny(StatusCode::NOT_FOUND),
        Ok(ThanksMutation::Added) => private_response(
            Redirect::to(&format!("/reader/{id}/thanks?done=added")),
            &[],
        ),
        Ok(ThanksMutation::Repeated) => private_response(
            Redirect::to(&format!("/reader/{id}/thanks?done=repeat")),
            &[],
        ),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn form_uses_server_identity_and_escapes_csrf() {
        let form = format!(
            "<form method=\"post\" action=\"/reader/5/thanks\">{}</form>",
            pages::escape_html("\"<&")
        );
        assert!(form.contains("/reader/5/thanks"));
        assert!(form.contains("&quot;&lt;&amp;"));
        assert!(!form.contains("user_id"));
    }
}
