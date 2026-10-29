//! User account pages; preserves the original `pages` public entry points.
use super::{escape_html, layout};
use axum::response::Html;
use tor_discussions::repositories::public_book_comments::ReceivedReply;

pub fn registration_form(csrf: &str) -> Html<String> {
    registration_form_with_error(csrf, "", "", None)
}

/// Use server-side validation instead of the browser's locale-dependent
/// English popups. Keep the email/name, but never replay the password.
pub fn registration_form_with_error(
    csrf: &str,
    display_name: &str,
    email: &str,
    error: Option<&str>,
) -> Html<String> {
    let banner = error
        .map(|message| {
            format!(
                "<p class=\"auth-form__error\" role=\"alert\">{}</p>",
                escape_html(message)
            )
        })
        .unwrap_or_default();
    let content = include_str!("../../templates/register_active.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--DISPLAY_NAME-->", &escape_html(display_name))
        .replace("<!--EMAIL-->", &escape_html(email))
        .replace("<!--FORM_ERROR-->", &banner)
        .replace(
            "<!--AUTH_PANDA-->",
            include_str!("../../templates/auth_panda.html"),
        );
    layout("Регистрация", "Создать личный кабинет читателя.", &content)
}

pub fn login_form(csrf: &str) -> Html<String> {
    let content = include_str!("../../templates/login_active.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace(
            "<!--AUTH_PANDA-->",
            include_str!("../../templates/auth_panda.html"),
        );
    layout("Вход", "Войти в личный кабинет.", &content)
}

/// Branded, standalone error page: only fixed site links, no account data or
/// database diagnostics. The caller keeps the original HTTP error status.
pub fn account_error_page(title: &str, message: &str, retry_registration: bool) -> Html<String> {
    let (primary_href, primary_label, secondary_href, secondary_label) = if retry_registration {
        ("/register", "Повторить регистрацию", "/login", "Войти")
    } else {
        ("/login", "Попробовать снова", "/register", "Регистрация")
    };
    let body = include_str!("../../templates/account_error.html")
        .replace("<!--ERROR_HEADING-->", &escape_html(title))
        .replace("<!--ERROR_MESSAGE-->", &escape_html(message))
        .replace("<!--PRIMARY_HREF-->", primary_href)
        .replace("<!--PRIMARY_LABEL-->", primary_label)
        .replace("<!--SECONDARY_HREF-->", secondary_href)
        .replace("<!--SECONDARY_LABEL-->", secondary_label);
    layout(
        "Ошибка учётной записи",
        "Сообщение об ошибке входа или регистрации.",
        &body,
    )
}

pub fn my_account(name: &str, csrf: &str, replies: &[ReceivedReply]) -> Html<String> {
    let content = include_str!("../../templates/me.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--RECEIVED_REPLIES-->", &received_replies(replies));
    // Only the authenticated private page changes its navigation.
    let html = layout("Мой кабинет", "Учётная запись и личный кабинет.", &content);
    Html(
        html.0
            .replace(
                "<a class=\"site-nav__link\" href=\"/login\">Войти</a>",
                "<a class=\"site-nav__link\" href=\"/me\" aria-current=\"page\">Мой кабинет</a>",
            )
            .replace(
                "<a class=\"button button--small\" href=\"/register\">Регистрация</a>",
                "",
            ),
    )
}

fn received_replies(replies: &[ReceivedReply]) -> String {
    if replies.is_empty() {
        return "<p class=\"ui-section-header__text\">Ответов пока нет. Когда другой читатель ответит на ваш комментарий, здесь появится ссылка на обсуждение.</p>".to_owned();
    }
    let mut html = String::from("<div class=\"me-now-list\">");
    for reply in replies.iter().take(10) {
        // The existing descending cursor includes this exact reply on arrival.
        let cursor = reply
            .comment_id
            .checked_add(1)
            .map(|after| format!("?after={after}"))
            .unwrap_or_default();
        html.push_str(&format!(
            "<a class=\"me-now-item me-now-item--reading\" href=\"/books/{}/comments{cursor}#comment-{}\">",
            reply.book_id, reply.comment_id,
        ));
        html.push_str(&format!(
            "<span class=\"me-now-item__copy\"><strong class=\"me-now-item__title\">Ответ в книге «{}»</strong><span class=\"me-now-item__text\">Открыть ответ на ваш комментарий</span></span><span class=\"me-now-item__arrow\" aria-hidden=\"true\">→</span></a>",
            escape_html(&reply.book_title),
        ));
    }
    html.push_str("</div>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_03_received_reply_links_escape_titles_and_contain_no_message_copy() {
        let replies = vec![ReceivedReply {
            comment_id: 26,
            book_id: 4,
            book_title: "<img src=x onerror=alert(1)>".to_owned(),
        }];
        let html = my_account("Reader", "csrf", &replies).0;
        assert!(html.contains("/books/4/comments?after=27#comment-26"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!html.contains("<img src=x"));
        assert!(!html.contains("<!--RECEIVED_REPLIES-->"));
        assert!(my_account("Reader", "csrf", &[])
            .0
            .contains("Ответов пока нет"));
    }

    #[test]
    fn restored_ne_wboo_auth_panda_is_present_without_client_script() {
        for html in [
            registration_form("csrf-value").0,
            login_form("csrf-value").0,
        ] {
            assert!(html.contains("auth-card auth-card--newboo"));
            assert!(html.contains("auth-panda__svg"));
            assert!(html.contains("viewBox=\"0 0 260 190\""));
            assert!(html.contains("auth-panda__cover-arms"));
            assert!(!html.contains("<!--AUTH_PANDA-->"));
            assert!(!html.contains("<script"));
        }
    }

    #[test]
    fn russian_validation_remains_server_side_and_never_returns_password() {
        let html = registration_form_with_error(
            "csrf-value",
            "Читатель <b>",
            "reader@example.test",
            Some("Пароль должен содержать от 12 до 128 символов"),
        )
        .0;
        assert!(html.contains("novalidate"));
        assert!(html.contains("name=\"csrf_token\" value=\"csrf-value\""));
        assert!(html.contains("Пароль должен содержать от 12 до 128 символов"));
        assert!(html.contains("Читатель &lt;b&gt;"));
        assert!(!html.contains("Читатель <b>"));
        assert!(html.contains("autocomplete=\"new-password\""));
        assert!(!html.contains("value=\"1234\""));
    }

    #[test]
    fn original_dashboard_sections_keep_only_implemented_routes() {
        let html = my_account("<Читатель>", "csrf-token", &[]).0;
        assert!(html.contains("class=\"me-page\""));
        assert!(html.contains("class=\"me-overview\""));
        assert!(html.contains("class=\"me-block-grid\""));
        assert!(html.contains("class=\"dashboard-tile dashboard-tile--reader\""));
        assert!(html.contains("Работа с книгами"));
        assert!(html.contains("Чтение"));
        assert!(html.contains("Сейчас"));
        for route in ["/me/library", "/me/books", "/me/books/new", "/catalog"] {
            assert!(html.contains(&format!("href=\"{route}\"")));
        }
        assert!(html.contains("name=\"csrf_token\" value=\"csrf-token\""));
        assert!(html.contains("&lt;Читатель&gt;"));
        assert!(!html.contains("<Читатель>"));
        assert!(!html.contains("Приватность"));
        assert!(!html.contains("персонального анализа"));
        assert!(!html.contains("href=\"/me/settings\""));
        assert!(!html.contains("href=\"/me/purchases\""));
        assert!(!html.contains("href=\"/me/finance\""));
    }
}
