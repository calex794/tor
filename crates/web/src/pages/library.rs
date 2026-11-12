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
        !updates.items.is_empty(),
        None,
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
        !updates.items.is_empty(),
        None,
    )
}

/// Purchases remain independent from bookmarks; tab selection changes presentation only.
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
    my_library_with_updates_and_layers_and_purchased_only_selected(
        items, saved, csrf, availability, checkout_enabled, updates_and_owned, purchased_only, None,
    )
}

pub fn my_library_with_updates_and_layers_and_purchased_only_selected(
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
    view: Option<&str>,
) -> Html<String> {
    let (updates, owned) = updates_and_owned;
    let (purchased_available, layers_available) = availability;
    let updates_html = update_cards(updates, checkout_enabled, purchased_available, owned, csrf);
    let purchased_only_html = purchase_only_book_cards(purchased_only, purchased_available);
    my_library_inner(
        items, saved, csrf, purchased_available, layers_available,
        &updates_html, &purchased_only_html, !updates.items.is_empty(), view,
    )
}

/// A public cover is attempted only for books without private deletion state.
fn book_art(book_id: i64, public_visible: bool) -> String {
    let image = if public_visible && book_id > 0 {
        format!("<img src=\"/covers/{book_id}/card.webp\" alt=\"\" loading=\"lazy\" decoding=\"async\">")
    } else {
        String::new()
    };
    format!("<div class=\"library-book-art\" aria-hidden=\"true\"><span class=\"library-book-art__spine\"></span><span class=\"library-book-art__glyph\">✦</span>{image}</div>")
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
        let art = book_art(book.book_id, !book.ordinary_deleted && !book.pending_deletion);
        write!(
            cards,
            "<article class=\"library-shelf-card\">{art}<div class=\"library-shelf-card__body\"><h3>{title}</h3>{status}{navigation}</div></article>"
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
         <div class=\"library-books-grid\">{cards}</div>{next}</section>"
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
    has_updates: bool,
    view: Option<&str>,
) -> Html<String> {
    let mut cards = String::new();
    if items.is_empty() {
        cards.push_str("<div class=\"library-placeholder\"><h2>Вы ещё не сохраняли место чтения</h2><p>Выберите книгу и сохраните позицию в главе, чтобы продолжить позже.</p><a class=\"button button--small\" href=\"/catalog\">Выбрать книгу</a></div>");
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
            let art = book_art(item.book_id, !item.archived_saved);
            write!(cards, "<article class=\"library-reading-card\">{art}<div class=\"library-reading-card__copy\"><span class=\"library-status\">{mode}</span><h3>{book}</h3><p>Глава {}. {chapter}</p><p class=\"library-card__place\">{hint}</p></div><a class=\"button\" href=\"{target}\">Продолжить</a></article>", item.chapter_no).expect("write HTML");
        }
    }
    let token = escape_html(csrf);
    let mut saved_html = String::new();
    if saved.items.is_empty() {
        saved_html.push_str("<div class=\"library-placeholder\"><h2>Пока нет сохранённых книг</h2><p>Добавьте книгу в личную библиотеку из каталога.</p><a class=\"button button--small\" href=\"/catalog\">Открыть каталог</a></div>");
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
            let art = book_art(book.book_id, !book.ordinary_deleted && !book.pending_deletion);
            write!(
                saved_html,
                "<article class=\"library-shelf-card\">{art}<div class=\"library-shelf-card__body\">{title_view}{state}<a href=\"/me/library/{}/saved\">Сохранённые главы</a></div><form method=\"post\" action=\"/me/library/{}/remove\"><input type=\"hidden\" name=\"csrf_token\" value=\"{token}\"><button class=\"library-remove\" type=\"submit\">Убрать из библиотеки</button></form></article>",
                book.book_id, book.book_id
            )
            .expect("writing saved book");
        }
    }
    if let Some(after) = saved.next_cursor {
        write!(
            saved_html,
            "<p class=\"library-cursor\"><a href=\"/me/library?after={after}&amp;view=saved\">Другие сохранённые книги</a></p>"
        )
        .expect("writing library cursor");
    }
    let no_content = items.is_empty() && saved.items.is_empty()
        && !has_updates && purchased_only_html.is_empty();
    let requested = match view {
        Some("saved") => "saved",
        Some("updates") => "updates",
        Some("purchased") => "purchased",
        _ => "reading",
    };
    let selected = if requested == "purchased" && !purchased_available
        && purchased_only_html.is_empty() { "reading" } else { requested };
    let tabs = [
        ("reading", "Читаю"),
        ("saved", "Сохранено"),
        ("updates", "Обновления"),
        ("purchased", "Куплено"),
    ];
    let mut nav = String::new();
    for (key, label) in tabs {
        let current = if key == selected { " aria-current=\"page\"" } else { "" };
        write!(nav, "<a class=\"library-tab\" href=\"/me/library?view={key}\"{current}>{label}</a>")
            .expect("write library nav");
    }
    let main = if no_content {
        "<div class=\"library-onboarding\"><div class=\"library-onboarding__art\" aria-hidden=\"true\"><span></span><span></span><span></span></div><h2>Ваша книжная полка пока пуста</h2><p>Начните с интересной книги. Здесь будут сохранённые истории, новые главы и место, на котором вы остановились.</p><a class=\"button\" href=\"/catalog\">Найти книгу</a></div>".to_owned()
    } else {
        match selected {
            "saved" => format!("<div class=\"library-pane\" id=\"saved-books\"><div class=\"library-pane-head\"><h2>Сохранённые книги</h2></div><div class=\"library-books-grid\">{saved_html}</div></div>"),
            "updates" => updates_html.to_owned(),
            "purchased" => if purchased_only_html.is_empty() {
                "<div class=\"library-placeholder\"><h2>Отдельных книг с покупками пока нет</h2><p>Книги, добавленные вручную, находятся на вкладке «Сохранено». История покупок доступна отдельно.</p></div>".to_owned()
            } else {
                purchased_only_html.to_owned()
            },
            _ => format!("<div class=\"library-pane\" id=\"continue-reading\"><div class=\"library-pane-head\"><h2>Продолжить чтение</h2></div><div class=\"library-reading-list\">{cards}</div></div>"),
        }
    };
    let purchased_link = if purchased_available {
        "<a href=\"/me/purchases\">Мои купленные главы</a>"
    } else { "" };
    let layers_link = if layers_available {
        "<a href=\"/me/library/layers\">Мои пакеты доступа</a>"
    } else { "" };
    let html = format!(
        "<section class=\"section library-v2\"><div class=\"container library-shell\">\
         <nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\"><a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span><span aria-current=\"page\">Библиотека</span></nav>\
         <div class=\"library-topline\"><div><p class=\"library-kicker\">Моё чтение</p><h1>Моя библиотека</h1><p class=\"library-intro\">Истории, к которым хочется возвращаться</p></div><div class=\"library-top-actions\"><a class=\"button button--small\" href=\"/catalog\">Найти книгу</a></div></div>\
         <div class=\"library-quick-links\">{purchased_link}{layers_link}<a href=\"/library/shared\">Рекомендации друзей</a></div>\
         <nav class=\"library-tabs\" aria-label=\"Разделы библиотеки\">{nav}</nav>{main}</div></section>"
    );
    layout("Моя библиотека", "Продолжить чтение в личной библиотеке.", &html)
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
        body.push_str("<div class=\"library-placeholder\"><h2>Новых глав пока нет</h2><p>После подписки на книгу или автора новые публикации появятся здесь.</p></div>");
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
            let art = book_art(item.book_id, true);
            write!(
                body,
                "<article class=\"library-update-card\">{art}<div class=\"library-update-card__copy\"><span class=\"library-status\">{state} · {read_hint}</span><h3>{book}</h3><p>Глава {}. {chapter}</p></div><div class=\"library-card__actions\">{action}{read_control}</div></article>",
                item.chapter_number,
            )
            .expect("writing safe HTML");
        }
    }
    let next = updates
        .next_cursor
        .map(|cursor| {
            format!(
                "<p class=\"library-cursor\"><a href=\"/me/library?updates_after={}&amp;view=updates#new-chapters\">Ранее вышедшие главы</a></p>",
                cursor.encode()
            )
        })
        .unwrap_or_default();
    format!(
        "<section class=\"section\" id=\"new-chapters\" aria-labelledby=\"new-chapters-heading\">\
         <h2 id=\"new-chapters-heading\">Новые главы подписок</h2>\
         <p>Здесь показываются бесплатные и платные главы после подписки на книгу или автора.\
         Подписка не даёт доступа к платному тексту.</p>\
         <div class=\"library-update-list\">{body}</div>{next}</section>"
    )
}

#[cfg(test)]
#[path = "library/workspace/tests.rs"]
mod workspace_tests;

#[cfg(test)]
#[path = "library/update/tests.rs"]
mod update_tests;

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
