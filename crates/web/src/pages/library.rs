//! Signed-in reading library and book-follow release feed.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;

pub fn my_library_with_updates(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
    purchased_available: bool,
    checkout_enabled: bool,
    notifications: (
        &tor_reading::repositories::follow_updates::FollowedReleasePage,
        &tor_reading::repositories::staged_updates::StagedRevisionPage,
    ),
    owned: &[tor_commerce::repositories::entitlements::ReadablePurchaseLink],
) -> Html<String> {
    let (updates, revisions) = notifications;
    let update_html = update_cards(updates, checkout_enabled, purchased_available, owned, csrf);
    let revision_html = revision_cards(revisions, csrf);
    my_library_inner(
        items,
        saved,
        csrf,
        purchased_available,
        &format!("{update_html}{revision_html}"),
    )
}

fn my_library_inner(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
    purchased_available: bool,
    updates_html: &str,
) -> Html<String> {
    let mut cards = String::new();
    if items.is_empty() {
        cards.push_str("<div class=\"library-empty\"><h2>Вы пока не сохраняли место чтения</h2><p>Откройте каталог, чтобы выбрать книгу. Откройте любую бесплатную главу, затем нажмите «Сохранить место здесь». После этого книга появится в вашей истории чтения.</p><a class=\"button\" href=\"/catalog\">Открыть каталог</a></div>");
    } else {
        for item in items {
            let book = escape_html(&item.book_title);
            let chapter = escape_html(&item.chapter_title);
            let bookmark = if item.block_index > 0 {
                format!("#p-{}", item.block_index)
            } else {
                String::new() // published version changed: never resume at an old anchor
            };
            let hint = if item.block_index > 0 {
                format!("Фрагмент {}", item.block_index)
            } else {
                "Глава обновлена — проверьте новое содержание".to_owned()
            };
            write!(cards, "<article class=\"library-card\"><div><p class=\"eyebrow\">Продолжить чтение</p><h2>{book}</h2><p>Глава {}. {chapter}</p><p class=\"library-card__place\">{hint}</p></div><a class=\"button\" href=\"/read/{}{bookmark}\">Продолжить</a></article>", item.chapter_no, item.chapter_id).expect("write HTML");
        }
    }
    let token = escape_html(csrf);
    let mut saved_html = String::new();
    if saved.items.is_empty() {
        saved_html.push_str("<p>Пока нет сохранённых книг.</p>");
    } else {
        for book in &saved.items {
            let title = escape_html(&book.title);
            write!(
                saved_html,
                "<article class=\"library-card\"><div><h3><a href=\"/books/{}\">{title}</a></h3></div><form method=\"post\" action=\"/me/library/{}/remove\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button class=\"button button--small\" type=\"submit\">Убрать из библиотеки</button></form></article>",
                book.book_id, book.book_id
            )
            .expect("writing saved book");
        }
    }
    if let Some(after) = saved.next_cursor {
        write!(
            saved_html,
            "<p><a href=\"/me/library?after={after}\">Другие сохранённые книги</a></p>"
        )
        .expect("writing library cursor");
    }
    let purchased_link = if purchased_available {
        "<p><a class=\"button button--small\" href=\"/me/purchases\">Мои купленные главы</a></p>"
    } else {
        ""
    };
    let html = format!(
        "<section class=\"section library-page\"><div class=\"container\"><nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\"><a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span><span aria-current=\"page\">Библиотека</span></nav><div class=\"library-heading\"><p class=\"eyebrow\">Моё чтение</p><h1>Продолжить чтение</h1><p>История чтения показывает бесплатные главы; новые главы подписок могут быть платными.</p>{purchased_link}</div><div class=\"library-grid\">{cards}</div>{updates_html}<section class=\"section\" aria-label=\"Сохранённые книги\"><h2>Сохранённые книги</h2><div class=\"library-grid\">{saved_html}</div></section></div></section>"
    );
    layout(
        "Моя библиотека",
        "Продолжить чтение в личной библиотеке.",
        &html,
    )
}

fn update_cards(
    updates: &tor_reading::repositories::follow_updates::FollowedReleasePage,
    checkout_enabled: bool,
    paid_reader_available: bool,
    owned: &[tor_commerce::repositories::entitlements::ReadablePurchaseLink],
    csrf: &str,
) -> String {
    let mut body = String::new();
    if updates.items.is_empty() {
        body.push_str("<p>После подписки новые главы появятся здесь.</p>");
    } else {
        for item in &updates.items {
            let book = escape_html(&item.book_title);
            let chapter = escape_html(&item.chapter_title);
            let purchased = owned.iter().find(|row| row.chapter_id == item.chapter_id);
            let (state, link, button) = if item.is_paid {
                let price = format!("{},{:02} ₽", item.price_minor / 100, item.price_minor % 100);
                if let Some(purchase) = purchased {
                    if paid_reader_available {
                        (
                            format!("Купленная глава · {price}"),
                            format!(
                                "/read/paid/{}?purchase_id={}",
                                item.chapter_id, purchase.purchase_id
                            ),
                            "Читать купленную главу",
                        )
                    } else {
                        (
                            format!("Купленная глава · {price}"),
                            String::new(),
                            "Чтение временно недоступно",
                        )
                    }
                } else if checkout_enabled {
                    (
                        format!("Платная глава · {price}"),
                        format!("/me/books/{}/buy-chapters", item.book_id),
                        "Перейти к покупке",
                    )
                } else {
                    (
                        format!("Платная глава · {price}"),
                        format!("/reader/{}", item.chapter_id),
                        "Условия доступа",
                    )
                }
            } else {
                (
                    "Бесплатная глава".to_owned(),
                    format!("/reader/{}", item.chapter_id),
                    "Читать",
                )
            };
            let action = if link.is_empty() {
                format!("<span class=\"library-card__place\">{button}</span>")
            } else {
                format!("<a class=\"button button--small\" href=\"{link}\">{button}</a>")
            };
            let (read_state, read_label, read_hint) = if item.is_read {
                ("unread", "Отметить непрочитанной", "Просмотрено")
            } else {
                ("read", "Отметить прочитанной", "Новое")
            };
            let read_control = format!(
                "<form method=\"post\" action=\"/me/updates/{}/read-state\">\
                 <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
                 <input type=\"hidden\" name=\"state\" value=\"{read_state}\">\
                 <button class=\"button button--small\" type=\"submit\">{read_label}</button>\
                 </form>",
                item.chapter_id,
                escape_html(csrf),
            );
            write!(
                body,
                "<article class=\"library-card\"><div><p class=\"eyebrow\">{state} · {read_hint}</p>\\
                 <h3>{book}</h3><p>Глава {}. {chapter}</p></div>\\
                 <div class=\"library-card__actions\">{action}{read_control}</div></article>",
                item.chapter_number,
            )
            .expect("writing safe HTML");
        }
    }
    let next = updates
        .next_cursor
        .map(|cursor| {
            format!(
                "<p><a href=\"/me/library?updates_after={}#new-chapters\">Ранее вышедшие главы</a></p>",
                cursor.encode()
            )
        })
        .unwrap_or_default();
    format!(
        "<section class=\"section\" id=\"new-chapters\" aria-labelledby=\"new-chapters-heading\">\
         <h2 id=\"new-chapters-heading\">Новые главы подписок</h2>\
         <p>Здесь показываются бесплатные и платные главы после подписки на книгу.\
         Подписка не даёт доступа к платному тексту.</p>\
         <div class=\"library-grid\">{body}</div>{next}</section>"
    )
}

/// V1 releases remain in C-16; only durably staged V2+ revisions appear here.
fn revision_cards(
    revisions: &tor_reading::repositories::staged_updates::StagedRevisionPage,
    csrf: &str,
) -> String {
    if revisions.items.is_empty() {
        return String::new();
    }
    let mut cards = String::new();
    for item in &revisions.items {
        let book = escape_html(&item.book_title);
        let chapter = escape_html(&item.chapter_title);
        let access = if item.is_paid {
            "Платная редакция · доступ к тексту отдельно"
        } else {
            "Бесплатная редакция"
        };
        let (state, label, read_hint) = if item.is_read {
            ("unread", "Отметить непрочитанной", "Просмотрено")
        } else {
            ("read", "Отметить прочитанной", "Новое")
        };
        let event_id = escape_html(&item.event_id);
        let token = escape_html(csrf);
        write!(
            cards,
            "<article class=\"library-card\"><div>\
             <p class=\"eyebrow\">{access} · V{} · {read_hint}</p>\
             <h3>{book}</h3><p>Глава {}. {chapter}</p></div>\
             <div class=\"library-card__actions\">\
             <a class=\"button button--small\" href=\"/reader/{}\">Открыть главу</a>\
             <form method=\"post\" action=\"/me/revisions/{event_id}/read-state\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{token}\">\
             <input type=\"hidden\" name=\"state\" value=\"{state}\">\
             <button class=\"button button--small\" type=\"submit\">{label}</button>\
             </form></div></article>",
            item.version_no, item.chapter_no, item.chapter_id,
        )
        .expect("writing safe revision HTML");
    }
    let next = revisions
        .next_cursor
        .as_ref()
        .map_or_else(String::new, |cursor| {
            format!(
                "<p><a href=\"/me/library?revisions_after={}#revised-chapters\">\
             Ранее обновлённые редакции</a></p>",
                cursor.encode()
            )
        });
    format!(
        "<section class=\"section\" id=\"revised-chapters\" \
         aria-labelledby=\"revised-chapters-heading\">\
         <h2 id=\"revised-chapters-heading\">Обновлённые редакции</h2>\
         <p>Изменённые главы подписок. Подписка не даёт доступа к платному тексту.</p>\
         <div class=\"library-grid\">{cards}</div>{next}</section>"
    )
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use tor_reading::repositories::follow_updates::{
        FollowedRelease, FollowedReleasePage, UpdatesCursor,
    };
    use tor_reading::repositories::staged_updates::{
        RevisionCursor, StagedRevision, StagedRevisionPage,
    };

    #[test]
    fn staged_revision_escapes_titles_and_leaks_no_paid_body() {
        let page = StagedRevisionPage {
            items: vec![StagedRevision {
                book_id: 9,
                chapter_id: 12,
                book_title: "<script>unsafe</script>".into(),
                chapter_title: "A & B".into(),
                chapter_no: 3,
                version_no: 2,
                is_paid: true,
                is_read: false,
                staged_micros: 1_800_000_000_123_456,
                event_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
            }],
            next_cursor: Some(RevisionCursor {
                staged_micros: 1_800_000_000_123_456,
                event_id: "01234567-89ab-cdef-0123-456789abcdef".into(),
            }),
        };
        let html = revision_cards(&page, "csrf-token");
        assert!(html.contains("Обновлённые редакции"));
        assert!(html.contains("&lt;script&gt;unsafe&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("A &amp; B"));
        assert!(html.contains("href=\"/reader/12\""));
        assert!(html
            .contains("action=\"/me/revisions/01234567-89ab-cdef-0123-456789abcdef/read-state\""));
        assert!(html.contains("name=\"csrf_token\" value=\"csrf-token\""));
        assert!(html.contains("name=\"state\" value=\"read\""));
        assert!(html.contains("Отметить прочитанной"));
        let mut read_page = page.clone();
        read_page.items[0].is_read = true;
        let read_html = revision_cards(&read_page, "csrf-token");
        assert!(read_html.contains("name=\"state\" value=\"unread\""));
        assert!(read_html.contains("Просмотрено"));
        assert!(html.contains("revisions_after=1800000000123456."));
        assert!(!html.contains("purchase_id="));
        let empty = revision_cards(
            &StagedRevisionPage {
                items: vec![],
                next_cursor: None,
            },
            "csrf-token",
        );
        assert!(empty.is_empty(), "no duplicate C-16 first-release section");
    }

    #[test]
    fn paid_update_is_escaped_and_points_to_enabled_checkout() {
        let page = FollowedReleasePage {
            items: vec![FollowedRelease {
                book_id: 9,
                chapter_id: 12,
                book_title: "<script>title</script>".into(),
                chapter_title: "A & B".into(),
                chapter_number: 2,
                is_paid: true,
                price_minor: 19900,
                is_read: false,
                released_micros: 123,
            }],
            next_cursor: Some(UpdatesCursor {
                released_micros: 123,
                chapter_id: 12,
            }),
        };
        let enabled = update_cards(&page, true, true, &[], "csrf-token");
        assert!(enabled.contains("/me/books/9/buy-chapters"));
        assert!(enabled.contains("199,00 ₽"));
        assert!(enabled.contains("&lt;script&gt;title"));
        assert!(enabled.contains("A &amp; B"));
        assert!(!enabled.contains("<script>"));
        assert!(enabled.contains("updates_after=123-12"));
        assert!(enabled.contains("action=\"/me/updates/12/read-state\""));
        assert!(enabled.contains("value=\"csrf-token\""));
        assert!(enabled.contains("value=\"read\""));
        assert!(enabled.contains("Новое"));
        let disabled = update_cards(&page, false, true, &[], "csrf-token");
        assert!(!disabled.contains("/me/books/9/buy-chapters"));
        assert!(disabled.contains("/reader/12"));
        let owned = [
            tor_commerce::repositories::entitlements::ReadablePurchaseLink {
                chapter_id: 12,
                purchase_id: 77,
            },
        ];
        let purchased = update_cards(&page, true, true, &owned, "csrf-token");
        assert!(purchased.contains("/read/paid/12?purchase_id=77"));
        assert!(!purchased.contains("/me/books/9/buy-chapters"));
        let unavailable = update_cards(&page, true, false, &owned, "csrf-token");
        assert!(unavailable.contains("Чтение временно недоступно"));
        assert!(!unavailable.contains("/read/paid/12"));
        assert!(!unavailable.contains("/me/books/9/buy-chapters"));
        let mut acknowledged = page.clone();
        acknowledged.items[0].is_read = true;
        let seen = update_cards(&acknowledged, true, true, &owned, "csrf-token");
        assert!(seen.contains("value=\"unread\""));
        assert!(seen.contains("Просмотрено"));
        assert!(seen.contains("Отметить непрочитанной"));
    }
}
