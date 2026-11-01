//! User account pages; preserves the original `pages` public entry points.
use super::{escape_html, layout};
use axum::response::Html;
use tor_discussions::repositories::public_book_comments::ReceivedReply;
use tor_recommendations::repositories::ReaderAchievements;

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

/// Private settings page: no password is returned in HTML or client-side scripts.
pub fn password_change_form(csrf: &str, message: Option<&str>) -> Html<String> {
    let banner = message
        .map(|text| {
            format!(
                "<p class=\"auth-form__error\" role=\"alert\">{}</p>",
                escape_html(text)
            )
        })
        .unwrap_or_default();
    let body = include_str!("../../templates/password_change.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--FORM_ERROR-->", &banner);
    layout(
        "Смена пароля",
        "Настройки безопасности учётной записи.",
        &body,
    )
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

#[allow(dead_code)] // Legacy renderer retained for existing callers and snapshot tests.
pub fn my_account(name: &str, csrf: &str, replies: &[ReceivedReply]) -> Html<String> {
    my_account_with_follow_updates(name, csrf, replies, 0)
}

/// Show a bounded private alert only for new, unacknowledged releases.
pub fn my_account_with_follow_updates(
    name: &str,
    csrf: &str,
    replies: &[ReceivedReply],
    unread: u16,
) -> Html<String> {
    render_my_account(name, csrf, replies, unread, None)
}

#[allow(dead_code)] // Kept for legacy callers and inline tests.
pub fn my_account_with_achievements(
    name: &str,
    csrf: &str,
    replies: &[ReceivedReply],
    unread: u16,
    achievements: &ReaderAchievements,
) -> Html<String> {
    render_my_account(name, csrf, replies, unread, Some(achievements))
}

pub fn my_account_with_achievements_and_swap(
    name: &str,
    csrf: &str,
    replies: &[ReceivedReply],
    unread: u16,
    achievements: &ReaderAchievements,
    swap_enabled: bool,
) -> Html<String> {
    let html = render_my_account(name, csrf, replies, unread, Some(achievements));
    let link = if swap_enabled {
        "<a class=\"dashboard-nav__item\" href=\"/me/refunds/swap\">Обмен глав</a>"
    } else {
        ""
    };
    Html(html.0.replace("<!--SWAP_NAV_LINK-->", link))
}

fn render_my_account(
    name: &str,
    csrf: &str,
    replies: &[ReceivedReply],
    unread: u16,
    achievements: Option<&ReaderAchievements>,
) -> Html<String> {
    let count = if unread >= 100 {
        "99+".to_owned()
    } else {
        unread.to_string()
    };
    let alert = if unread == 0 {
        String::new()
    } else {
        format!(
            "<a class=\"me-now-item me-now-item--reading\" href=\"/me/library#new-chapters\" \
             aria-label=\"Новые главы подписок: {count}\"><span class=\"me-now-item__copy\">\
             <strong class=\"me-now-item__title\">Новые главы подписок: {count}</strong>\
             <span class=\"me-now-item__text\">Бесплатные и платные продолжения ваших книг.</span>\
             </span><span class=\"me-now-item__arrow\" aria-hidden=\"true\">→</span></a>"
        )
    };
    let content = include_str!("../../templates/me.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--FOLLOWED_RELEASE_NOTICE-->", &alert)
        .replace("<!--RECEIVED_REPLIES-->", &received_replies(replies, csrf))
        .replace(
            "<!--READER_ACHIEVEMENTS-->",
            &reader_achievements(achievements),
        );
    private_account_layout("Мой кабинет", "Учётная запись и личный кабинет.", &content)
}

/// Switch public navigation to the authenticated account shell.
pub(super) fn private_account_layout(
    title: &str,
    description: &str,
    content: &str,
) -> Html<String> {
    let html = layout(title, description, content);
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

fn reader_achievements(snapshot: Option<&ReaderAchievements>) -> String {
    let Some(snapshot) = snapshot else {
        return "<p class=\"ui-section-header__text\">Достижения появятся после подтверждённого чтения.</p>"
            .to_owned();
    };
    let mut cards = String::from("<div class=\"me-block-grid\">");
    for badge in &snapshot.badges {
        let definition = badge.definition;
        let done = badge.unlocked_on.is_some();
        let status = if done {
            "Получено"
        } else {
            "Пока не получено"
        };
        let progress = snapshot.completed_chapters.min(definition.threshold);
        cards.push_str(&format!(
            "<article class=\"dashboard-tile dashboard-tile--reader\">\
             <h3 class=\"dashboard-tile__title\">{}</h3>\
             <p class=\"dashboard-tile__text\">{}</p>\
             <p class=\"dashboard-tile__text\">{status}: {progress}/{} глав</p>",
            escape_html(definition.title),
            escape_html(definition.description),
            definition.threshold,
        ));
        if let Some(unlocked_on) = &badge.unlocked_on {
            cards.push_str(&format!(
                "<p class=\"dashboard-tile__text\">Дата получения: {}</p>",
                escape_html(unlocked_on),
            ));
        }
        cards.push_str("</article>");
    }
    cards.push_str("</div>");
    cards
}

fn received_replies(replies: &[ReceivedReply], csrf: &str) -> String {
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
        let (status, next_state, action) = if reply.is_read {
            ("Прочитано", "unread", "Отметить непрочитанным")
        } else {
            ("Не прочитано", "read", "Отметить прочитанным")
        };
        html.push_str(&format!(
            "<p class=\"ui-section-header__text\">{status}</p><form method=\"post\" action=\"/me/replies/{}/read-state\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><input type=\"hidden\" name=\"state\" value=\"{next_state}\"><button class=\"ui-button ui-button--ghost ui-button--small\" type=\"submit\">{action}</button></form>",
            reply.comment_id, escape_html(csrf),
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
            is_read: false,
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
    fn c_16_dashboard_links_to_unread_free_and_paid_release_updates() {
        let html = my_account_with_follow_updates("Reader", "csrf", &[], 2).0;
        assert!(html.contains("href=\"/me/library#new-chapters\""));
        assert!(html.contains("Новые главы подписок: 2"));
        assert!(html.contains("Бесплатные и платные"));
        let big = my_account_with_follow_updates("Reader", "csrf", &[], 100).0;
        assert!(big.contains("Новые главы подписок: 99+"));
        assert!(!my_account_with_follow_updates("Reader", "csrf", &[], 0)
            .0
            .contains("href=\"/me/library#new-chapters\""));
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
    fn f_12_dashboard_shows_only_verified_private_reader_badges() {
        use tor_recommendations::repositories::ReaderBadge;
        use tor_recommendations::services::achievements::READER_ACHIEVEMENTS_V1;
        let achievements = ReaderAchievements {
            completed_chapters: 1,
            badges: vec![
                ReaderBadge {
                    definition: READER_ACHIEVEMENTS_V1[0],
                    unlocked_on: Some("2026-10-31".to_owned()),
                },
                ReaderBadge {
                    definition: READER_ACHIEVEMENTS_V1[1],
                    unlocked_on: None,
                },
            ],
        };
        let page = my_account_with_achievements("Reader", "csrf", &[], 0, &achievements).0;
        assert!(page.contains("Достижения"));
        assert!(page.contains("Первая глава"));
        assert!(page.contains("Получено: 1/1 глав"));
        assert!(page.contains("Пока не получено: 1/10 глав"));
        assert!(page.contains("Дата получения: 2026-10-31"));
        assert!(!page.contains("<!--READER_ACHIEVEMENTS-->"));
        assert!(!page.contains("source_event_id"));
        assert!(!my_account("Reader", "csrf", &[])
            .0
            .contains("Получено: 1/1 глав"));
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
    #[test]
    fn d_06_swap_link_requires_explicit_browser_opt_in() {
        let achievements = ReaderAchievements {
            completed_chapters: 0,
            badges: Vec::new(),
        };
        let hidden =
            my_account_with_achievements_and_swap("Reader", "csrf", &[], 0, &achievements, false).0;
        assert!(!hidden.contains("href=\"/me/refunds/swap\""));
        let enabled =
            my_account_with_achievements_and_swap("Reader", "csrf", &[], 0, &achievements, true).0;
        assert!(enabled.contains("href=\"/me/refunds/swap\""));
    }
}
