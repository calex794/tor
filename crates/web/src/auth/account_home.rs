//! Authenticated account home. Routing and RIL registration stay in auth.rs.
use super::*;

pub(super) async fn show(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(Redirect::to("/login"), &[]);
    };
    let digest = session_token_digest(&token);
    let verified = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    page(
        state,
        headers,
        verified.actor.user_id.get(),
        verified.administrator,
    )
    .await
}

pub(super) async fn page(
    state: AuthState,
    headers: HeaderMap,
    user_id: i64,
    administrator: bool,
) -> Response {
    let name = match state.accounts.user_display_name(user_id).await {
        Ok(Some(value)) => value,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let discussions =
        tor_discussions::repositories::public_book_comments::PgPublicBookComments::new(
            state.accounts.pool().clone(),
        );
    let replies = match discussions.received_replies(user_id, 10).await {
        Ok(replies) => replies,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let unread = match state.follow_updates.unread_count_capped(user_id).await {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let achievements = match state.achievements.snapshot(user_id).await {
        Ok(achievements) => achievements,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut page = pages::my_account_with_achievements_refunds_and_swap(
        &name,
        &csrf,
        &replies,
        unread,
        &achievements,
        state.swap_browser_enabled,
        false,
    );
    let audience_link = if administrator {
        "<a class=\"dashboard-nav__item\" href=\"/admin/audience\">Аудитория</a>"
    } else {
        ""
    };
    page.0 = page.0.replace("<!--AUDIENCE_NAV_LINK-->", audience_link);
    private_response(page, &cookies)
}
