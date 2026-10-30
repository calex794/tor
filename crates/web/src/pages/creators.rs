//! E-05: private, keyset-paged followed creators. No public follower listing.
use super::{account::private_account_layout, escape_html};
use axum::response::Html;
use tor_social::services::follow::FollowedCreator;

pub fn creator_follows(
    creators: &[FollowedCreator],
    next: Option<i64>,
    csrf: &str,
) -> Html<String> {
    let mut cards = String::from("<ul class=\"creator-follow-list\">");
    if creators.is_empty() {
        cards.push_str("<li>Вы пока не подписаны на авторов. Откройте книгу и нажмите «Следить за автором».</li>");
    }
    for creator in creators {
        cards.push_str(&format!(
            "<li class=\"dashboard-tile\"><strong>{}</strong><p>Подписка с {} (UTC)</p>",
            escape_html(&creator.display_name),
            escape_html(&creator.started_day),
        ));
        if let Some((book_id, title)) = &creator.source_book {
            cards.push_str(&format!(
                "<p>Источник: <a class=\"text-link\" href=\"/books/{book_id}\">{}</a></p>",
                escape_html(title),
            ));
        }
        cards.push_str(&format!(
            "<form method=\"post\" action=\"/me/creators/{}/unfollow\">\
              <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
              <button class=\"ui-button ui-button--ghost\" type=\"submit\">Отписаться от автора</button>\
             </form></li>",
            creator.creator_user_id,
            escape_html(csrf),
        ));
    }
    cards.push_str("</ul>");
    if let Some(cursor) = next {
        cards.push_str(&format!(
            "<p><a class=\"text-link\" href=\"/me/creators?after={cursor}\">Следующая страница</a></p>"
        ));
    }
    let content = format!(
        "<section class=\"me-page\"><div class=\"container\">\
           <h1>Подписки на авторов</h1>\
           <p>Здесь показаны ваши активные подписки. Повторная подписка после отписки начинает новый срок.</p>\
           <p><a href=\"/me\">Мой кабинет</a></p>\
           {cards}\
         </div></section>"
    );
    private_account_layout("Подписки на авторов", "Ваши подписки на аккаунты авторов.", &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_05_follower_list_escapes_source_names_and_csrf() {
        let page = creator_follows(
            &[FollowedCreator {
                period_id: 2,
                creator_user_id: 17,
                display_name: "<script>Harmful".to_owned(),
                started_day: "2026-10-31".to_owned(),
                source_book: Some((28, "<img src=x>".to_owned())),
            }],
            Some(2),
            "csrf\"><script>",
        )
        .0;
        assert!(page.contains("&lt;script&gt;Harmful"));
        assert!(page.contains("&lt;img src=x&gt;"));
        assert!(page.contains("href=\"/books/28\""));
        assert!(page.contains("href=\"/me/creators?after=2\""));
        assert!(page.contains("action=\"/me/creators/17/unfollow\""));
        assert!(!page.contains("<script>Harmful"));
        assert!(!page.contains("value=\"csrf\"><script>"));
    }
}
