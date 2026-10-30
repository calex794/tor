//! S052: authenticated self-service referral link and privacy-safe aggregates.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tor_accounts::services::security::generate_session_token;
use tor_referrals::repositories::postgres::ReferralOverview;

#[derive(Deserialize)]
pub(super) struct CreateReferralForm {
    csrf_token: Option<String>,
    book_id: Option<u64>,
}

#[derive(Default, Deserialize)]
pub(super) struct ReferralPageQuery {
    book: Option<u64>,
    alias_error: Option<String>,
}

#[derive(Serialize)]
struct Invitation {
    code: String,
    status: String,
    url: Option<String>,
    short_url: Option<String>,
}

#[derive(Serialize)]
struct ReferralReport {
    invitation: Option<Invitation>,
    active: i64,
    under_review: i64,
    blocked: i64,
    expired: i64,
}

fn report_data(overview: ReferralOverview, origin: &str) -> ReferralReport {
    let invitation = overview.default_code.map(|owned| {
        let url = (owned.status == "active")
            .then(|| format!("{}/r/{}", origin.trim_end_matches('/'), owned.code));
        let short_url = if owned.status == "active" {
            owned
                .alias_path_prefix
                .as_deref()
                .zip(owned.alias.as_deref())
                .map(|(prefix, alias)| format!("{}/{prefix}/{alias}", origin.trim_end_matches('/')))
        } else {
            None
        };
        Invitation {
            code: owned.code,
            status: owned.status,
            url,
            short_url,
        }
    });
    ReferralReport {
        invitation,
        active: overview.active,
        under_review: overview.under_review,
        blocked: overview.blocked,
        expired: overview.expired,
    }
}

async fn overview(state: &AuthState, actor: i64) -> Result<ReferralOverview, StatusCode> {
    state
        .referrals
        .reader_overview(actor)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Query(query): Query<ReferralPageQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let snapshot = match overview(&state, actor).await {
        Ok(snapshot) => snapshot,
        Err(status) => return deny(status),
    };
    // An arbitrary guessed book ID must not produce a link to a hidden title.
    if let Some(id) = query.book {
        match state.reader.public_book_page(id, 1, None).await {
            Ok(Some(_)) => {}
            Ok(None) => return deny(StatusCode::NOT_FOUND),
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::referrals_page(
            &csrf,
            &state.cookies.origin,
            &snapshot,
            query.book,
            query.alias_error.as_deref(),
        ),
        &cookies,
    )
}

async fn create_for_actor(state: &AuthState, actor: i64) -> Result<(), StatusCode> {
    let random_code = format!("r_{}", generate_session_token());
    state
        .referrals
        .create_default_reader_code(actor, &random_code)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

pub(super) async fn create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<CreateReferralForm>,
) -> Response {
    // Keep the session and CSRF evidence in the public handler. The
    // repository receives only an authenticated actor ID.
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match create_for_actor(&state, actor).await {
        Ok(()) => {
            let location = form
                .book_id
                .and_then(tor_search::domain::seo_urls::BookUrl::from_id)
                .map(|book| format!("/me/referrals?book={}", book.id()))
                .unwrap_or_else(|| "/me/referrals".to_owned());
            private_response(Redirect::to(&location), &[])
        }
        Err(status) => deny(status),
    }
}

pub(super) async fn api_get(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match overview(&state, actor).await {
        Ok(snapshot) => private_response(Json(report_data(snapshot, &state.cookies.origin)), &[]),
        Err(status) => deny(status),
    }
}

pub(super) async fn api_create(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<CreateReferralForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    if let Err(status) = create_for_actor(&state, actor).await {
        return deny(status);
    }
    match overview(&state, actor).await {
        Ok(snapshot) => private_response(Json(report_data(snapshot, &state.cookies.origin)), &[]),
        Err(status) => deny(status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_referrals::repositories::postgres::OwnedReferralCode;

    #[test]
    fn f_09_api_uses_configured_origin_and_hides_disabled_links() {
        let report = ReferralOverview {
            default_code: Some(OwnedReferralCode {
                code: "r_abc123".to_owned(),
                status: "active".to_owned(),
                alias: None,
                alias_path_prefix: None,
            }),
            active: 2,
            under_review: 0,
            blocked: 1,
            expired: 0,
        };
        let payload = report_data(report.clone(), "https://books.example/");
        assert_eq!(
            payload.invitation.unwrap().url.unwrap(),
            "https://books.example/r/r_abc123"
        );
        let mut disabled = report;
        disabled.default_code.as_mut().unwrap().status = "blocked".to_owned();
        assert!(report_data(disabled, "https://books.example")
            .invitation
            .unwrap()
            .url
            .is_none());
    }
}
