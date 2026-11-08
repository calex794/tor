//! Signed-in reading library and book/creator-follow release feed.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;

pub fn my_library_with_updates(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
    purchased_available: bool,
    checkout_enabled: bool,
    updates: &tor_reading::repositories::follow_updates::FollowedReleasePage,
    owned: &[tor_commerce::services::reader_views::ReadablePurchaseLink],
) -> Html<String> {
    let update_html = update_cards(updates, checkout_enabled, purchased_available, owned, csrf);
    my_library_inner(
        items,
        saved,
        csrf,
        purchased_available,
        false,
        &update_html,
        "",
    )
}

/// The purchased-Layer navigation only appears with explicit storefront opt-in.
pub fn my_library_with_updates_and_layers(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
    availability: (bool, bool),
    checkout_enabled: bool,
    updates: &tor_reading::repositories::follow_updates::FollowedReleasePage,
    owned: &[tor_commerce::services::reader_views::ReadablePurchaseLink],
) -> Html<String> {
    let (purchased_available, layers_available) = availability;
    if !layers_available {
        return my_library_with_updates(
            items,
            saved,
            csrf,
            purchased_available,
            checkout_enabled,
            updates,
            owned,
        );
    }
    let update_html = update_cards(updates, checkout_enabled, purchased_available, owned, csrf);
    my_library_inner(
        items,
        saved,
        csrf,
        purchased_available,
        layers_available,
        &update_html,
        "",
    )
}

/// A purchase is separate from a bookmark. Keep bought-only Books in the
/// private shelf, without displaying a misleading "remove bookmark" action.
pub fn my_library_with_updates_and_layers_and_purchased_only(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
    availability: (bool, bool),
    checkout_enabled: bool,
    updates_and_owned: (
        &tor_reading::repositories::follow_updates::FollowedReleasePage,
        &[tor_commerce::services::reader_views::ReadablePurchaseLink],
    ),
    purchased_only: &tor_reading::services::library::PurchasedOnlyBookPage,
) -> Html<String> {
    let (updates, owned) = updates_and_owned;
    if purchased_only.items.is_empty() {
        return my_library_with_updates_and_layers(
            items,
            saved,
            csrf,
            availability,
            checkout_enabled,
            updates,
            owned,
        );
    }
    let (purchased_available, layers_available) = availability;
    let updates_html = update_cards(updates, checkout_enabled, purchased_available, owned, csrf);
    let purchased_only_html = purchase_only_book_cards(purchased_only, purchased_available);
    my_library_inner(
        items,
        saved,
        csrf,
        purchased_available,
        layers_available,
        &updates_html,
        &purchased_only_html,
    )
}

fn purchase_only_book_cards(
    page: &tor_reading::services::library::PurchasedOnlyBookPage,
    paid_reader_available: bool,
) -> String {
    if page.items.is_empty() {
        return String::new();
    }
    let mut cards = String::new();
    for book in &page.items {
        let title = escape_html(&book.title);
        let status = if book.ordinary_deleted {
            "<p class=\"library-card__place\" role=\"status\">Удалена</p>"
        } else if book.pending_deletion {
            "<p class=\"library-card__place\" role=\"status\">Ожидает удаления</p>"
        } else {
            ""
        };
        let navigation = if paid_reader_available {
            format!(
                "<p><a href=\"/me/library/{}/saved\">Купленные главы</a></p>",
                book.book_id
            )
        } else {
            "<p>Купленные главы — чтение временно недоступно</p>".to_owned()
        };
        write!(
            cards,
            "<article class=\"library-card\"><div><h3>{title}</h3>{status}{navigation}</div></article>"
        )
        .expect("write purchased-only book");
    }
    let next = page
        .next_cursor
        .map(|id| {
            format!(
                "<p><a href=\"/me/library?purchased_after={id}\">Другие книги с покупками</a></p>"
            )
        })
        .unwrap_or_default();
    format!(
        "<section class=\"section\" aria-label=\"Книги с покупками\">\
         <h2>Книги с покупками</h2>\
         <p>Ваши оплаченные главы остаются в библиотеке независимо от закладок.</p>\
         <div class=\"library-grid\">{cards}</div>{next}</section>"
    )
}

fn my_library_inner(
    items: &[tor_reading::services::progress::ContinueReading],
    saved: &tor_reading::services::library::SavedBookPage,
    csrf: &str,
    purchased_available: bool,
    layers_available: bool,
    updates_html: &str,
    purchased_only_html: &str,
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
            // Deleted Books never link back to the public /read endpoint.
            // The private pinned reader rechecks session, age and legal holds.
            let (target, mode) = if item.archived_saved {
                let anchor = if item.block_index > 0 {
                    "#saved-resume"
                } else {
                    ""
                };
                (
                    format!("/me/library/saved/{}{anchor}", item.chapter_id),
                    "Сохранённая редакция · Удалена",
                )
            } else {
                (
                    format!("/read/{}{bookmark}", item.chapter_id),
                    "Продолжить чтение",
                )
            };
            write!(cards, "<article class=\"library-card\"><div><p class=\"eyebrow\">{mode}</p><h2>{book}</h2><p>Глава {}. {chapter}</p><p class=\"library-card__place\">{hint}</p></div><a class=\"button\" href=\"{target}\">Продолжить</a></article>", item.chapter_no).expect("write HTML");
        }
    }
    let token = escape_html(csrf);
    let mut saved_html = String::new();
    if saved.items.is_empty() {
        saved_html.push_str("<p>Пока нет сохранённых книг.</p>");
    } else {
        for book in &saved.items {
            let title = escape_html(&book.title);
            // A saved Book is still present after ordinary deletion.
            // Its public detail URL is gone; use only the private pinned reader.
            let state = if book.ordinary_deleted {
                "<p class=\"library-card__place\" role=\"status\">Удалена</p>"
            } else if book.pending_deletion {
                "<p class=\"library-card__place\" role=\"status\">Ожидает удаления</p>"
            } else {
                ""
            };
            let title_view = if book.ordinary_deleted {
                format!("<h3>{title}</h3>")
            } else {
                format!("<h3><a href=\"/books/{}\">{title}</a></h3>", book.book_id)
            };
            write!(
                saved_html,
                "<article class=\"library-card\"><div>{title_view}{state}<p><a href=\"/me/library/{}/saved\">Сохранённые главы</a></p></div><form method=\"post\" action=\"/me/library/{}/remove\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button class=\"button button--small\" type=\"submit\">Убрать из библиотеки</button></form></article>",
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
    let layers_link = if layers_available {
        "<p><a class=\"button button--small\" href=\"/me/library/layers\">Мои пакеты доступа</a></p>"
    } else {
        ""
    };
    let html = format!(
        "<section class=\"section library-page\"><div class=\"container\"><nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\"><a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span><span aria-current=\"page\">Библиотека</span></nav><div class=\"library-heading\"><p class=\"eyebrow\">Моё чтение</p><h1>Продолжить чтение</h1><p>История чтения показывает бесплатные главы; новые главы подписок могут быть платными.</p>{purchased_link}{layers_link}</div><div class=\"library-grid\">{cards}</div>{updates_html}<section class=\"section\" aria-label=\"Сохранённые книги\"><h2>Сохранённые книги</h2><div class=\"library-grid\">{saved_html}</div></section>{purchased_only_html}</div></section>"
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
    owned: &[tor_commerce::services::reader_views::ReadablePurchaseLink],
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
                // The private followed-books feed is for an authenticated reader:
                // use the progress/completion-capable route, not the public preview.
                (
                    "Бесплатная глава".to_owned(),
                    format!("/read/{}", item.chapter_id),
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
         <p>Здесь показываются бесплатные и платные главы после подписки на книгу или автора.\
         Подписка не даёт доступа к платному тексту.</p>\
         <div class=\"library-grid\">{body}</div>{next}</section>"
    )
}

#[cfg(test)]
mod update_tests {
    use super::*;
    use tor_reading::repositories::follow_updates::{
        FollowedRelease, FollowedReleasePage, UpdatesCursor,
    };

    #[test]
    fn free_update_opens_the_authenticated_completion_capable_reader() {
        let page = FollowedReleasePage {
            items: vec![FollowedRelease {
                book_id: 9,
                chapter_id: 12,
                book_title: "Книга".into(),
                chapter_title: "Новая бесплатная глава".into(),
                chapter_number: 2,
                is_paid: false,
                price_minor: 0,
                is_read: false,
                released_micros: 123,
            }],
            next_cursor: None,
        };
        let html = update_cards(&page, false, false, &[], "csrf-token");
        assert!(html.contains("href=\"/read/12\""));
        assert!(!html.contains("href=\"/reader/12\""));
        assert!(html.contains("action=\"/me/updates/12/read-state\""));
        assert!(html.contains("Новая бесплатная глава"));
        assert!(html.contains("Отметить прочитанной"));
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
        let owned = [tor_commerce::services::reader_views::ReadablePurchaseLink {
            chapter_id: 12,
            purchase_id: 77,
        }];
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

#[cfg(test)]
mod pending_library_tests {
    use super::*;
    use tor_reading::{
        repositories::follow_updates::FollowedReleasePage,
        services::library::{SavedBook, SavedBookPage},
    };

    #[test]
    fn pending_deletion_book_is_private_visible_and_distinct_from_deleted() {
        let saved = SavedBookPage {
            items: vec![
                SavedBook {
                    cursor_id: 22,
                    book_id: 55,
                    title: "Тест <script>alert(1)</script>".into(),
                    pending_deletion: true,
                    ordinary_deleted: false,
                },
                SavedBook {
                    cursor_id: 21,
                    book_id: 56,
                    title: "Обычная книга".into(),
                    pending_deletion: false,
                    ordinary_deleted: false,
                },
            ],
            next_cursor: None,
        };
        let updates = FollowedReleasePage {
            items: Vec::new(),
            next_cursor: None,
        };
        let page = my_library_with_updates(&[], &saved, "csrf-token", false, false, &updates, &[]);
        let html = page.0;
        assert!(html.contains("Ожидает удаления"));
        assert_eq!(html.matches("Ожидает удаления").count(), 1);
        assert!(!html.contains(">Удалена<"));
        assert!(html.contains("href=\"/books/55\""));
        assert!(html.contains("action=\"/me/library/55/remove\""));
        assert!(html.contains("href=\"/me/library/55/saved\""));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }
}

#[cfg(test)]
mod archived_continue_tests {
    use super::*;
    use tor_reading::{
        repositories::follow_updates::FollowedReleasePage,
        services::{library::SavedBookPage, progress::ContinueReading},
    };

    #[test]
    fn archived_free_v1_resume_never_links_to_public_reader() {
        let saved = SavedBookPage {
            items: vec![],
            next_cursor: None,
        };
        let updates = FollowedReleasePage {
            items: vec![],
            next_cursor: None,
        };
        let items = [ContinueReading {
            book_id: 44,
            book_title: "Удалённая <книга>".into(),
            chapter_id: 71,
            chapter_no: 2,
            chapter_title: "Сохранённая глава".into(),
            block_index: 3,
            archived_saved: true,
        }];
        let html = my_library_with_updates(&items, &saved, "csrf", false, false, &updates, &[]).0;
        assert!(html.contains("href=\"/me/library/saved/71#saved-resume\""));
        assert!(html.contains("Сохранённая редакция · Удалена"));
        assert!(html.contains("&lt;книга&gt;"));
        assert!(!html.contains("href=\"/read/71"));

        let public = [ContinueReading {
            archived_saved: false,
            ..items[0].clone()
        }];
        let current =
            my_library_with_updates(&public, &saved, "csrf", false, false, &updates, &[]).0;
        assert!(current.contains("href=\"/read/71#p-3\""));
        assert!(!current.contains("href=\"/me/library/saved/71"));
    }
}

#[cfg(test)]
mod purchase_only_book_tests {
    use super::*;
    use tor_reading::{
        repositories::follow_updates::FollowedReleasePage,
        services::library::{PurchasedOnlyBook, PurchasedOnlyBookPage, SavedBookPage},
    };

    #[test]
    fn bought_only_deleted_book_stays_in_library_without_remove_bookmark_action() {
        let bookmarked = SavedBookPage {
            items: vec![],
            next_cursor: None,
        };
        let only_bought = PurchasedOnlyBookPage {
            items: vec![
                PurchasedOnlyBook {
                    book_id: 88,
                    title: "Removed <script>alert(1)</script>".into(),
                    pending_deletion: false,
                    ordinary_deleted: true,
                },
                PurchasedOnlyBook {
                    book_id: 77,
                    title: "Pending".into(),
                    pending_deletion: true,
                    ordinary_deleted: false,
                },
            ],
            next_cursor: Some(77),
        };
        let updates = FollowedReleasePage {
            items: vec![],
            next_cursor: None,
        };
        let html = my_library_with_updates_and_layers_and_purchased_only(
            &[],
            &bookmarked,
            "csrf",
            (true, false),
            false,
            (&updates, &[]),
            &only_bought,
        )
        .0;
        assert!(html.contains("aria-label=\"Книги с покупками\""));
        assert!(html.contains("href=\"/me/library/88/saved\""));
        assert!(html.contains("href=\"/me/library/77/saved\""));
        assert!(html.contains("href=\"/me/library?purchased_after=77\""));
        assert!(html.contains("Удалена"));
        assert!(html.contains("Ожидает удаления"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("action=\"/me/library/88/remove\""));
        assert!(!html.contains("action=\"/me/library/77/remove\""));

        let unavailable = my_library_with_updates_and_layers_and_purchased_only(
            &[],
            &bookmarked,
            "csrf",
            (false, false),
            false,
            (&updates, &[]),
            &only_bought,
        )
        .0;
        assert!(unavailable.contains("чтение временно недоступно"));
        assert!(!unavailable.contains("href=\"/me/library/88/saved\""));
    }
}
