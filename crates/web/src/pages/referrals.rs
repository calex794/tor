//! S052: private referral dashboard, without earnings or invited identities.
use super::{account::private_account_layout, escape_html};
use axum::response::Html;
use tor_referrals::repositories::postgres::ReferralOverview;

pub fn referrals_page(
    csrf: &str,
    origin: &str,
    overview: &ReferralOverview,
    book_id: Option<u64>,
    alias_error: Option<&str>,
) -> Html<String> {
    let invitation = match &overview.default_code {
        Some(owned) if owned.status == "active" => {
            let url = format!("{}/r/{}", origin.trim_end_matches('/'), owned.code);
            format!(
                "<label for=\"referral-url\">Ваша ссылка</label>\
                 <input class=\"ui-input\" type=\"text\" id=\"referral-url\" \
                 aria-label=\"Ваша реферальная ссылка\" readonly value=\"{}\">\
                 <p class=\"ui-section-header__text\">Скопируйте ссылку и передайте её читателю.</p>",
                escape_html(&url)
            )
        }
        Some(_) => "<p class=\"ui-section-header__text\" role=\"status\">\
                     Приглашения по вашей ссылке сейчас недоступны. \
                     Создать новую ссылку вместо заблокированной нельзя.</p>"
            .to_owned(),
        None => format!(
            "<p class=\"ui-section-header__text\">У вас пока нет ссылки для приглашений.</p>\
             <form method=\"post\" action=\"/me/referrals\">\
               <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
               {}\
               <button class=\"ui-button ui-button--primary\" type=\"submit\">Создать ссылку</button>\
             </form>",
            escape_html(csrf),
            book_id.map(|id| format!(
                "<input type=\"hidden\" name=\"book_id\" value=\"{id}\">"
            )).unwrap_or_default()
        ),
    };
    let book_share = match (book_id, &overview.default_code) {
        (Some(id), Some(owned)) if owned.status == "active" => {
            let link = format!(
                "{}/books/{id}?ref={}",
                origin.trim_end_matches('/'),
                owned.code
            );
            format!(
                "<section class=\"me-section\" aria-label=\"Ссылка на книгу\">\
                 <h2 class=\"ui-section-header__title\">Поделиться книгой</h2>\
                 <label for=\"book-referral-url\">Ссылка на публичную книгу</label>\
                 <input type=\"text\" id=\"book-referral-url\" readonly \
                 aria-label=\"Ссылка для приглашения на книгу\" value=\"{}\">\
                 <p class=\"ui-section-header__text\">Книга остаётся публичной; \
                 ссылка не открывает платные главы.</p></section>",
                escape_html(&link)
            )
        }
        (Some(_), _) => "<p class=\"ui-section-header__text\">\
                         Сначала создайте активную реферальную ссылку.</p>"
            .to_owned(),
        (None, _) => String::new(),
    };
    let alias_notice = match alias_error {
        Some("invalid") => "<p role=\"alert\">Недопустимое короткое имя или тип ссылки.</p>",
        Some("taken") => "<p role=\"alert\">Короткое имя уже занято.</p>",
        Some("locked") => "<p role=\"alert\">Опубликованный короткий адрес изменить нельзя.</p>",
        Some("unavailable") => "<p role=\"alert\">Сначала нужен активный код приглашения.</p>",
        _ => "",
    };
    let alias_panel = match &overview.default_code {
        Some(code) if code.status == "active" => {
            match (code.alias_path_prefix.as_deref(), code.alias.as_deref()) {
                (Some(prefix), Some(alias)) => {
                    let url = format!("{}/{prefix}/{alias}", origin.trim_end_matches('/'));
                    format!(
                        "<section class=\"me-section\" aria-label=\"Короткая ссылка\">\
                         <h2 class=\"ui-section-header__title\">Короткая ссылка</h2>\
                         <label for=\"referral-short-url\">Ваш неизменяемый адрес</label>\
                         <input class=\"ui-input\" type=\"text\" id=\"referral-short-url\" \
                         aria-label=\"Ваша короткая ссылка\" readonly value=\"{}\">\
                         <p class=\"ui-section-header__text\">Адрес привязан к коду навсегда. \
                         Изменить его после публикации нельзя.</p></section>",
                        escape_html(&url)
                    )
                }
                _ => format!(
                    "<section class=\"me-section\" aria-label=\"Создать короткую ссылку\">\
                     <h2 class=\"ui-section-header__title\">Короткая ссылка</h2>{alias_notice}\
                     <form method=\"post\" action=\"/me/referrals/alias\">\
                       <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
                       <label for=\"referral-alias-prefix\">Тип короткой ссылки</label>\
                       <select id=\"referral-alias-prefix\" name=\"prefix\">\
                         <option value=\"go\">/go/</option>\
                         <option value=\"promo\">/promo/</option>\
                       </select>\
                       <label for=\"referral-alias\">Короткое имя ссылки</label>\
                       <input id=\"referral-alias\" class=\"ui-input\" type=\"text\" name=\"alias\" \
                         required minlength=\"3\" maxlength=\"64\" \
                         pattern=\"[a-z0-9][a-z0-9-]{{2,63}}\" \
                         autocomplete=\"off\" placeholder=\"kniga-2026\">\
                       <button type=\"submit\" class=\"ui-button ui-button--primary\">\
                         Создать короткую ссылку</button>\
                     </form>\
                     <p class=\"ui-section-header__text\">Выбор окончательный. \
                       Регистрация по ссылке не обещает вознаграждение.</p></section>",
                    escape_html(csrf)
                ),
            }
        }
        _ => String::new(),
    };
    let body = format!(
        "<section class=\"me-page\"><div class=\"container\">\
           <nav class=\"dashboard-nav\" aria-label=\"Разделы кабинета\">\
             <a class=\"dashboard-nav__item\" href=\"/me\">Обзор</a>\
             <a class=\"dashboard-nav__item is-active\" href=\"/me/referrals\" aria-current=\"page\">Приглашения</a>\
           </nav>\
           <header class=\"ui-page-header\"><div>\
             <p class=\"ui-page-header__kicker\">Личный кабинет</p>\
             <h1 class=\"ui-page-header__title\">Мои приглашения</h1>\
             <p class=\"ui-page-header__text\">Делитесь ссылкой и отслеживайте привязанные регистрации.</p>\
           </div></header>\
           <section class=\"me-section\" aria-label=\"Реферальная ссылка\">{invitation}</section>\
           {book_share}\
           {alias_panel}\
           <section class=\"me-section\" aria-label=\"Атрибуция регистраций\">\
             <h2 class=\"ui-section-header__title\">Атрибуция</h2>\
             <p>Активных привязок: <strong>{}</strong></p>\
             <p>На проверке: <strong>{}</strong></p>\
             <p>Заблокировано: <strong>{}</strong></p>\
             <p>Истекло: <strong>{}</strong></p>\
             <p class=\"ui-section-header__text\">Это только учёт приглашений. \
                Регистрация, результативное чтение и финансовые начисления — разные события. \
                Привязка не обещает вознаграждение.</p>\
           </section>\
         </div></section>",
        overview.active, overview.under_review, overview.blocked, overview.expired,
    );
    private_account_layout(
        "Мои приглашения",
        "Личная реферальная ссылка и агрегированная атрибуция.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_referrals::repositories::postgres::OwnedReferralCode;

    #[test]
    fn f_09_dashboard_escapes_origin_and_never_lists_invited_users() {
        let overview = ReferralOverview {
            default_code: Some(OwnedReferralCode {
                code: "r_example".to_owned(),
                status: "active".to_owned(),
                alias: None,
                alias_path_prefix: None,
            }),
            active: 1,
            under_review: 0,
            blocked: 0,
            expired: 0,
        };
        let html = referrals_page(
            "csrf",
            "https://books.example/?x=<script>",
            &overview,
            None,
            None,
        )
        .0;
        assert!(html.contains("https://books.example/?x=&lt;script&gt;/r/r_example"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("Активных привязок: <strong>1</strong>"));
        assert!(!html.contains("subject_user_id"));
        let hidden = referrals_page(
            "csrf",
            "https://books.example",
            &ReferralOverview {
                default_code: Some(OwnedReferralCode {
                    code: "r_example".to_owned(),
                    status: "blocked".to_owned(),
                    alias: None,
                    alias_path_prefix: None,
                }),
                ..overview
            },
            Some(1),
            None,
        )
        .0;
        assert!(!hidden.contains("id=\"referral-url\""));
        assert!(hidden.contains("Создать новую ссылку вместо заблокированной нельзя"));
    }

    #[test]
    fn f_09_dashboard_creation_is_post_only_and_has_csrf() {
        let overview = ReferralOverview {
            default_code: None,
            active: 0,
            under_review: 0,
            blocked: 0,
            expired: 0,
        };
        let html = referrals_page(
            "csrf<secret>",
            "https://books.example",
            &overview,
            Some(15),
            None,
        )
        .0;
        assert!(html.contains("method=\"post\" action=\"/me/referrals\""));
        assert!(html.contains("name=\"csrf_token\" value=\"csrf&lt;secret&gt;\""));
        assert!(html.contains("Создать ссылку"));
        assert!(html.contains("name=\"book_id\" value=\"15\""));
        assert!(!html.contains("type=\"text\" id=\"referral-url\""));
    }

    #[test]
    fn f_09_public_book_link_contains_only_own_code() {
        let overview = ReferralOverview {
            default_code: Some(OwnedReferralCode {
                code: "r_example".to_owned(),
                status: "active".to_owned(),
                alias: None,
                alias_path_prefix: None,
            }),
            active: 1,
            under_review: 0,
            blocked: 0,
            expired: 0,
        };
        let html = referrals_page("csrf", "https://books.example", &overview, Some(12), None).0;
        assert!(html.contains("id=\"book-referral-url\""));
        assert!(html.contains("https://books.example/books/12?ref=r_example"));
        assert!(!html.contains("subject_user_id"));
        assert!(html.contains("name=\"alias\""));
        assert!(html.contains("name=\"prefix\""));
    }

    #[test]
    fn f_09_immutable_alias_is_visible_without_new_creation_form() {
        let overview = ReferralOverview {
            default_code: Some(OwnedReferralCode {
                code: "r_example".to_owned(),
                status: "active".to_owned(),
                alias: Some("summer-reads".to_owned()),
                alias_path_prefix: Some("promo".to_owned()),
            }),
            active: 0,
            under_review: 0,
            blocked: 0,
            expired: 0,
        };
        let html = referrals_page("csrf", "https://books.example", &overview, None, None).0;
        assert!(html.contains("https://books.example/promo/summer-reads"));
        assert!(!html.contains("action=\"/me/referrals/alias\""));
    }
}
