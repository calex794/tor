//! Public source-profile entry for direct mail and friendship; account lookup stays in tor-accounts.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(display_name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if display_name.is_empty() || display_name.chars().count() > 80 {
        return deny(StatusCode::NOT_FOUND);
    }
    let (recipient, name) = match state
        .accounts
        .public_profile_by_display_name(&display_name)
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let visitor = match session_user(&state, &headers).await {
        Ok(value) => value,
        Err(status) => return deny(status),
    };
    let mut action = String::new();
    let mut cookies = Vec::new();
    if let Some(actor) = visitor.filter(|id| *id != recipient) {
        use tor_social::repositories::friendships::{FriendState, PgFriendships};
        let state_view = match PgFriendships::new(state.accounts.pool().clone())
            .state(actor, recipient)
            .await
        {
            Ok(value) => value,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
        let (token, issued_cookies) = state.cookies.csrf_for_page(&headers);
        cookies = issued_cookies;
        let token = pages::escape_html(&token);
        // The source shows mail on logged-in, non-self profiles.
        // The server's block/send guard remains authoritative after a click.
        action.push_str(&format!("<form method=\"post\" action=\"/mail/start/{recipient}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button type=\"submit\" class=\"ui-button ui-button--ghost\">Почта</button></form>"));
        let friend_action = match state_view {
            FriendState::None => Some(("friend", "Добавить в друзья")),
            FriendState::Incoming => Some(("friend", "Принять в друзья")),
            FriendState::Friends => Some(("unfriend", "Удалить из друзей")),
            _ => None,
        };
        if let Some((suffix, label)) = friend_action {
            action.push_str(&format!("<form method=\"post\" action=\"/users/{recipient}/{suffix}\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button type=\"submit\">{label}</button></form>"));
        } else if state_view == FriendState::Outgoing {
            action.push_str("<p>Заявка уже отправлена.</p>");
        } else {
            action.push_str("<p>Действие недоступно: переписка заблокирована.</p>");
        }
        if !matches!(
            state_view,
            FriendState::BlockedByMe | FriendState::BlockedMe
        ) {
            action.push_str(&format!("<form method=\"post\" action=\"/users/{recipient}/block\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button type=\"submit\">Заблокировать</button></form>"));
        }
    }
    let html = pages::layout(
        "Публичный профиль",
        "Профиль участника.",
        &format!(
        "<section class=\"me-page\"><div class=\"container\"><h1>{}</h1>{action}</div></section>",
        pages::escape_html(&name)
    ),
    );
    private_response(html, &cookies)
}
