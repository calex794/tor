//! Authenticated book-detail follow controls. Never infer follows from library rows.
#![forbid(unsafe_code)]

use crate::auth_cookies::CookiePolicy;
use axum::http::{HeaderMap, StatusCode};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::repositories::postgres::PgAuthoring;
use tor_reading::repositories::PgBookFollows;
use tor_social::services::follow::PgCreatorFollows;

#[derive(Clone)]
pub(super) struct FollowBookControl {
    accounts: PgAccounts,
    follows: PgBookFollows,
    creator_follows: PgCreatorFollows,
    authoring: PgAuthoring,
    cookies: CookiePolicy,
}

impl FollowBookControl {
    pub(super) fn new(accounts: PgAccounts, secure: bool, origin: String) -> Self {
        let follows = PgBookFollows::new(accounts.pool().clone());
        let creator_follows = PgCreatorFollows::new(accounts.pool().clone());
        let authoring = PgAuthoring::new(accounts.pool().clone());
        Self {
            accounts,
            follows,
            creator_follows,
            authoring,
            cookies: CookiePolicy { secure, origin },
        }
    }

    /// Reuse the verified account session and cookie origin for public book capture.
    /// Keep referral writes in F-owned repositories, never the public HTML renderer.
    pub(super) fn browser_context(&self) -> (&PgAccounts, &CookiePolicy) {
        (&self.accounts, &self.cookies)
    }

    /// Only called after the public book visibility check succeeded.
    /// Anonymous readers see a login link, not a CSRF-less mutation form.
    pub(super) async fn render(
        &self,
        headers: &HeaderMap,
        book_id: u64,
    ) -> Result<(String, Vec<String>, bool), StatusCode> {
        let login = || {
            (
                "<a class=\"ui-button ui-button--ghost\" href=\"/login\">Следить за книгой</a>"
                    .to_owned(),
                Vec::new(),
                false,
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
        // B owns authorization. An authenticated session alone is not an
        // authoring right. The public page was already visibility-checked.
        let is_owner = self
            .authoring
            .owned_book(user_id, book_id_sql)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .is_some();
        let (csrf, cookies) = self.cookies.csrf_for_page(headers);
        let (action, label) = if following {
            ("unfollow", "Отписаться от книги")
        } else {
            ("follow", "Следить за книгой")
        };
        let mut html = format!(
            "<form method=\"post\" action=\"/me/books/{book_id}/{action}\" class=\"book-hero-actions__follow\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">{label}</button></form>"
        );
        // Creator identity comes from the B-owned row, never from a credit
        // string or client-supplied author id. A BookFollow remains separate.
        let creator = self
            .creator_follows
            .creator_for_public_book(book_id_sql)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if let Some(creator) = creator.filter(|owner| *owner != user_id) {
            let active = self
                .creator_follows
                .is_following(user_id, creator)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let (route, text) = if active {
                (format!("/me/creators/{creator}/unfollow"), "Отписаться от автора")
            } else {
                (format!("/me/books/{book_id}/follow-creator"), "Следить за автором")
            };
            html.push_str(&format!(
                "<form method=\"post\" action=\"{route}\" class=\"book-hero-actions__follow\"><input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\"><button class=\"ui-button ui-button--ghost\" type=\"submit\">{text}</button></form>"
            ));
        }
        Ok((html, cookies, is_owner))
    }
}

/// Inject controls only after the real PostgreSQL owner check. Markers are
/// generated from public TOC numeric IDs, never from submitted HTML.
fn personalize_book_detail(html: &str, book_id: u64, follow: &str, is_owner: bool) -> String {
    let owner_actions = if is_owner {
        format!(
            "<nav class=\"book-owner-actions\" aria-label=\"Управление книгой\">\
             <a class=\"ui-button ui-button--small\" href=\"/me/books/{book_id}/chapters/new\">Добавить главу</a>\
             <a class=\"ui-button ui-button--ghost ui-button--small\" href=\"/me/books/{book_id}/description\">Аннотация</a>\
             <a class=\"ui-button ui-button--ghost ui-button--small\" href=\"/me/books/{book_id}\">Управление книгой</a>\
             </nav>"
        )
    } else {
        String::new()
    };
    let content = html
        .replace("<!--BOOK_FOLLOW_ACTION-->", follow)
        .replace("<!--BOOK_OWNER_ACTIONS-->", &owner_actions);

    // Remove all owner-only per-chapter markers for guests and other users.
    // Do not put owner controls into an otherwise public cacheable page.
    const PREFIX: &str = "<!--BOOK_OWNER_EDIT:";
    const END: &str = "-->";
    let mut output = String::with_capacity(content.len());
    let mut input = content.as_str();
    while let Some(start) = input.find(PREFIX) {
        output.push_str(&input[..start]);
        input = &input[start + PREFIX.len()..];
        let Some(end) = input.find(END) else {
            // A malformed internal placeholder cannot grant author actions.
            return output;
        };
        let raw = &input[..end];
        if is_owner
            && raw.len() <= 20
            && !raw.is_empty()
            && raw.bytes().all(|value| value.is_ascii_digit())
        {
            if let Ok(chapter_id) = raw.parse::<u64>() {
                if chapter_id > 0 {
                    output.push_str(&format!(
                        "<a class=\"ui-button ui-button--ghost ui-button--small\" \
                         href=\"/me/books/{book_id}/chapters/{chapter_id}/edit\">Редактировать</a>"
                    ));
                }
            }
        }
        input = &input[end + END.len()..];
    }
    output.push_str(input);
    output
}

/// The public visibility gate runs before this function. Personal state is
/// never embedded into a shared-cacheable public response.
pub(super) async fn attach(
    control: &Option<FollowBookControl>,
    headers: &HeaderMap,
    book_id: u64,
    html: axum::response::Html<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Some(control) = control else {
        return axum::response::Html(personalize_book_detail(&html.0, book_id, "", false))
            .into_response();
    };
    match control.render(headers, book_id).await {
        Ok((action, cookies, is_owner)) => crate::auth_cookies::private_response(
            axum::response::Html(personalize_book_detail(&html.0, book_id, &action, is_owner)),
            &cookies,
        ),
        Err(status) => crate::auth_cookies::private_response(status, &[]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETAIL: &str = "<!--BOOK_FOLLOW_ACTION--><!--BOOK_OWNER_ACTIONS-->\
         <ol><li>Бесплатная<!--BOOK_OWNER_EDIT:12--></li>\
         <li>Платная</li></ol>";

    #[test]
    fn owner_can_edit_public_free_chapters_directly_from_the_book() {
        let html = personalize_book_detail(DETAIL, 77, "Следить", true);
        assert!(html.contains("Следить"));
        assert!(html.contains("/me/books/77/chapters/new"));
        assert!(html.contains("/me/books/77/chapters/12/edit"));
        assert!(html.contains("/me/books/77/description"));
        assert!(!html.contains("<!--BOOK_OWNER_"));
    }

    #[test]
    fn guest_and_non_owner_never_receive_authoring_links() {
        let html = personalize_book_detail(DETAIL, 77, "Следить", false);
        assert!(html.contains("Следить"));
        assert!(!html.contains("/me/books/77"));
        assert!(!html.contains("Редактировать"));
        assert!(!html.contains("<!--BOOK_OWNER_"));
    }

    #[test]
    fn malformed_internal_chapter_marker_is_not_an_author_link() {
        let html =
            personalize_book_detail("<!--BOOK_OWNER_EDIT:12%22onclick=alert(1)-->", 77, "", true);
        assert!(!html.contains("/edit"));
        assert!(!html.contains("onclick"));
    }
}
