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
    let revisions = match tor_social::repositories::notifications::PgNotifications::new(
        state.accounts.pool().clone(),
    )
    .unread_chapter_revisions_capped(user_id)
    .await
    {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let achievements = match state.achievements.snapshot(user_id).await {
        Ok(achievements) => achievements,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // The existing Library resolver is the sole source of Home resume links.
    // Failure is unavailable, never a fallback URL that could bypass access.
    let resumes = match state.progress.continue_reading(user_id).await {
        Ok(items) => items,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // Paid-only Books have no free cursor. Reuse Library's live D policy
    // (including session, age, deletion and refund gates) for their links.
    let paid_only = if state.paid_reader_available {
        let Some(session) = state.cookies.session_token(&headers) else {
            return deny(StatusCode::UNAUTHORIZED);
        };
        let digest = session_token_digest(&session);
        match state
            .bookmarks
            .purchased_only_books(user_id, &digest, 2, None)
            .await
        {
            Ok(page) => page.items,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        Vec::new()
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
        "<a class=\"dashboard-nav__item\" href=\"/odmen\">Администрирование</a>"
    } else {
        ""
    };
    page.0 = page.0.replace("<!--AUDIENCE_NAV_LINK-->", audience_link);
    page.0 = page.0.replace(
        "<!--RET_RESUME_CARDS-->",
        &pages::account_resume_cards(&resumes, &paid_only),
    );
    let changed = if revisions == 0 {
        String::new()
    } else {
        let count = if revisions >= 100 {
            "99+".to_owned()
        } else {
            revisions.to_string()
        };
        format!(
            "<a class=\"me-now-item me-now-item--reading\" \
             href=\"/me/library?view=updates#updated-chapters\" \
             aria-label=\"Обновились главы: {count}\">\
             <span class=\"me-now-item__copy\">\
             <strong class=\"me-now-item__title\">Обновились главы: {count}</strong>\
             <span class=\"me-now-item__text\">Новые редакции опубликованных глав.</span>\
             </span><span class=\"me-now-item__arrow\" aria-hidden=\"true\">→</span></a>"
        )
    };
    page.0 = page.0.replace("<!--FOLLOWED_REVISION_NOTICE-->", &changed);
    // Admin tools must be discoverable without an extra disclosure click.
    if administrator {
        page.0 = page.0.replacen(
            "<details class=\"account-nav-group\">",
            "<details class=\"account-nav-group\" open>",
            1,
        );
    }
    private_response(page, &cookies)
}
