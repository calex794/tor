//! Authenticated book-detail follow controls. Never infer follows from library rows.
#![forbid(unsafe_code)]

use crate::auth_cookies::CookiePolicy;
use axum::http::{HeaderMap, StatusCode};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::session_token_digest,
};
use tor_reading::repositories::PgBookFollows;

#[derive(Clone)]
pub(super) struct FollowBookControl {
    accounts: PgAccounts,
    follows: PgBookFollows,
    cookies: CookiePolicy,
}

impl FollowBookControl {
    pub(super) fn new(accounts: PgAccounts, secure: bool, origin: String) -> Self {
        let follows = PgBookFollows::new(accounts.pool().clone());
        Self {
            accounts,
            follows,
            cookies: CookiePolicy { secure, origin },
        }
    }

    /// Only called after the public book visibility check succeeded.
    /// Anonymous readers see a login link, not a CSRF-less mutation form.
    pub(super) async fn render(
        &self,
        headers: &HeaderMap,
        book_id: u64,
    ) -> Result<(String, Vec<String>), StatusCode> {
        let login = || {
            (
                "<a class=\"ui-button ui-button--ghost\" href=\"/login\">Следить за книгой</a>"
                    .to_owned(),
                Vec::new(),
            )
        };
        let Some(token) = self.cookies.session_token(headers) else {
            return Ok(login());
        };
        let user_id = self
            .accounts
            .active_session_user(&session_token_digest(&token))
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let Some(user_id) = user_id else {
            return Ok(login());
        };
        let book_id_sql = i64::try_from(book_id).map_err(|_| StatusCode::BAD_REQUEST)?;
        let following = self
            .follows
            .is_following(user_id, book_id_sql)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let (csrf, cookies) = self.cookies.csrf_for_page(headers);
        let (action, label) = if following {
            ("unfollow", "Отписаться от книги")
        } else {
            ("follow", "Следить за книгой")
        };
        let html = format!(
            "<form method=\"post\" action=\"/me/books/{book_id}/{action}\" class=\"book-hero-actions__follow\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">{label}</button></form>"
        );
        Ok((html, cookies))
    }
}
