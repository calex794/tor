//! User account pages; preserves the original `pages` public entry points.
use super::{escape_html, layout};
use axum::response::Html;

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

pub fn my_account(name: &str, csrf: &str) -> Html<String> {
    let content = include_str!("../../templates/me.html")
        .replace("<!--DISPLAY_NAME-->", &escape_html(name))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf));
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
