//! Public chapter reading and the S005 paid metadata paywall.
use super::{auth_cookies, pages, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{Redirect, Response},
};
use tor_accounts::services::security::session_token_digest;
use tor_commerce::repositories::admin_reader_entitlements::PgAdministratorReaderEntitlements;
use tor_contracts::paid_delivery::{Actor, EntitlementDecision, EntitlementPort, PositiveId};
use tor_discussions::repositories::public_chapter_comments::{ListError, PgPublicChapterComments};

/// A cookie is not an entitlement: PgReader rechecks session, account and
/// grant in the SAME statement that returns a chapter body or Book metadata.
pub(super) fn reader_session_digest(state: &AppState, headers: &HeaderMap) -> Option<[u8; 32]> {
    let (_, cookies) = state.follow_control.as_ref()?.browser_context();
    let token = cookies.session_token(headers)?;
    Some(session_token_digest(&token))
}

/// E-12: non-authoritative display-only issue result; no persistence on GET.
#[derive(Default, serde::Deserialize)]
pub(super) struct PublicIssueQuery {
    pub issue: Option<String>,
    pub comment: Option<String>,
    pub comments_after: Option<i64>,
}

/// Only a live A administrator with either an authorized unsealed current
/// edition or a sealed B/G paid edition may reach the private admin reader.
/// No role, version, purchase identity or private metadata comes from a query.
async fn administrator_private_redirect(
    state: &AppState,
    headers: &HeaderMap,
    id: u64,
) -> Result<Option<Response>, StatusCode> {
    if !state.private_admin_reader_enabled {
        return Ok(None);
    }
    let Some(control) = state.follow_control.as_ref() else {
        return Ok(None);
    };
    let (accounts, cookies) = control.browser_context();
    let Some(session) = cookies.session_token(headers) else {
        return Ok(None);
    };
    let digest = session_token_digest(&session);
    let actor = accounts
        .session_actor(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let Some(actor) = actor.filter(|actor| actor.administrator) else {
        return Ok(None);
    };
    let chapter_id = i64::try_from(id)
        .ok()
        .and_then(|raw| PositiveId::new(raw).ok());
    let Some(chapter_id) = chapter_id else {
        return Ok(None);
    };
    let actor_id =
        PositiveId::new(actor.actor.user_id.get()).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let authority = PgAdministratorReaderEntitlements::new(accounts.pool().clone(), digest);
    // Match the destination reader: prefer the current B working draft or
    // unsealed publication, then resolve a verified sealed paid edition.
    // Both operations recheck the live session/administrator role in SQL.
    let administrator = Actor { user_id: actor_id };
    let eligible = match authority.unsealed_version(administrator, chapter_id).await {
        Ok(Some(_)) => true,
        // Never redirect sealed paid content to a reader that has no
        // private S3 browser. Its admin entitlement is real but its bytes
        // are not deliverable in the current runtime configuration.
        Ok(None) if state.private_admin_sealed_reader_enabled => match authority
            .resolve_entitlement(administrator, chapter_id, None)
            .await
        {
            Ok(EntitlementDecision::Allowed { pinned }) if pinned.chapter_id == chapter_id => true,
            Ok(EntitlementDecision::Denied { .. }) => false,
            Ok(EntitlementDecision::Allowed { .. }) => return Err(StatusCode::CONFLICT),
            Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
        },
        Ok(None) => false,
        Err(_) => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !eligible {
        return Ok(None);
    }
    // Redirect only; actual plaintext or encrypted grant is authorized again
    // by the admin reader, so a role revoked mid-navigation cannot leak bytes.
    let mut response =
        auth_cookies::private_response(Redirect::to(&format!("/read/admin/{id}")), &[]);
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    Ok(Some(response))
}

/// C-10: successful and denied reading both use no-store and strict CSP.
pub(super) async fn reader_page(
    State(state): State<AppState>,
    Path(id): Path<u64>,
    Query(query): Query<PublicIssueQuery>,
    headers: HeaderMap,
    method: Method,
    observation: crate::editorial_http::display::Issuer,
) -> Response {
    let Some(reader) = state.reader.as_ref() else {
        return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]);
    };
    let digest = reader_session_digest(&state, &headers);
    let chapter = match reader.public_chapter_for_session(id, digest.as_ref()).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => {
            // NEwBOO parity: the same ordinary /reader link works for an
            // administrator on hidden/held sealed chapters and unsealed drafts.
            // Guests and ordinary readers retain the public paywall/404.
            match administrator_private_redirect(&state, &headers, id).await {
                Ok(Some(redirect)) => return redirect,
                Ok(None) => {}
                Err(status) => return auth_cookies::private_response(status, &[]),
            }
            match reader
                .public_paid_chapter_preview_for_session(id, digest.as_ref())
                .await
            {
                Ok(Some(preview)) => {
                    // Publicly visible paid metadata has a paywall, never plaintext.
                    // Hidden, held, deleted and unreleased chapters still yield 404.
                    let page = if state.paid_checkout_browser {
                        pages::paid_chapter_paywall_checkout(&preview, Some(id))
                    } else {
                        pages::paid_chapter_paywall(&preview)
                    };
                    let mut response = auth_cookies::private_response(page, &[]);
                    response.headers_mut().insert(
                        "x-robots-tag",
                        axum::http::HeaderValue::from_static("noindex, nofollow"),
                    );
                    return response;
                }
                Ok(None) => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
                Err(_) => {
                    return auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[])
                }
            }
        }
        Err(_) => return auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    if !tor_reading::domain::renderer::valid_reader_length(&chapter.body_markdown) {
        return auth_cookies::private_response(StatusCode::PAYLOAD_TOO_LARGE, &[]);
    }
    let mut progress_csrf = None;
    let mut progress_cookies = Vec::new();
    if method == Method::GET {
        if let Some(catalog) = state.postgres.as_ref() {
            // If verification is unavailable, omit optional analytics. Never
            // misclassify an authenticated reader as an anonymous visitor.
            let actor = match state.follow_control.as_ref() {
                Some(control) => match control.verified_reader(&headers).await {
                    Ok(actor) => actor,
                    Err(_) => {
                        return auth_cookies::private_response(
                            pages::reader_with_login_link(&chapter, Some(id)),
                            &[],
                        )
                    }
                },
                None => None,
            };
            // Only verified sessions may receive a CSRF pair for the existing
            // actor-bound PUT /api/v1/me/progress/free endpoint.
            if actor.is_some() {
                if let Some(control) = state.follow_control.as_ref() {
                    let (_, policy) = control.browser_context();
                    let (csrf, cookies) = policy.csrf_for_page(&headers);
                    progress_csrf = Some(csrf);
                    progress_cookies = cookies;
                }
            }
            if let Ok(chapter_id) = i64::try_from(id) {
                if let Err(error) =
                    tor_reading::repositories::book_stats::PgBookStats::new(catalog.pool())
                        .record_open(chapter_id, actor)
                        .await
                {
                    tracing::warn!(?error, "book statistics open not recorded");
                }
            }
        }
    }
    // E-02/S011: comments are scoped to the currently published *free*
    // edition. Querying the canonical E tables never inherits book-level or
    // purchased chapter visibility, and keyset reading stays bounded.
    let discussion = if let Some(catalog) = state.postgres.as_ref() {
        let (chapter_id, version_id) = match (i64::try_from(id), i64::try_from(chapter.version_id))
        {
            (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
            _ => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
        };
        let store = PgPublicChapterComments::new(catalog.pool().clone());
        match store
            .list_free_for_session(
                chapter_id,
                version_id,
                query.comments_after,
                25,
                digest.as_ref(),
            )
            .await
        {
            Ok(Some(comments)) => Some(comments),
            Ok(None) => return auth_cookies::private_response(StatusCode::NOT_FOUND, &[]),
            Err(ListError::Invalid) => {
                return auth_cookies::private_response(StatusCode::BAD_REQUEST, &[]);
            }
            Err(ListError::Unavailable) => {
                return auth_cookies::private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
            }
        }
    } else {
        None
    };
    let html = pages::reader_with_public_discussions(
        &chapter,
        Some(id),
        progress_csrf.as_deref(),
        query.issue.as_deref(),
        discussion.as_ref(),
        query.comment.as_deref(),
    );
    let html = if let Some(editorial) = &state.editorial {
        editorial
            .attach(
                &headers,
                tor_contracts::exposure::PromotionSlot::ChapterEndFree,
                i64::try_from(chapter.book_id).ok(),
                html,
                crate::editorial_http::display::issuer(&observation),
            )
            .await
    } else {
        html
    };
    let mut response = auth_cookies::private_response(html, &progress_cookies);
    // Never enable scripts for paid, hidden or denied content.
    response.headers_mut().insert(
        "content-security-policy",
        axum::http::HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    crate::recommendation_http::visits::book_response(response, chapter.book_id, 1)
}
