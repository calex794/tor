//! Private Home continuation links backed by the Library's canonical target.
use super::super::escape_html;

/// Private Home navigation uses exactly the same destination resolver as Library.
/// Only server-side actor-scoped progress is rendered; links never confer rights.
pub fn account_resume_cards(
    items: &[tor_reading::services::progress::ContinueReading],
    paid_only: &[tor_reading::services::library::PurchasedOnlyBook],
) -> String {
    let mut links = String::new();
    for item in items.iter().take(2) {
        let target = super::super::library::reading_destination(item);
        let book_title = escape_html(&item.book_title);
        let chapter_title = escape_html(&item.chapter_title);
        let status = if item.archived_saved {
            "Сохранённая редакция · Удалена"
        } else if item.block_index > 0 {
            "Сохранённая позиция"
        } else {
            "Новая или обновлённая глава"
        };
        links.push_str(&format!(
            "<a class=\"me-now-item me-now-item--reading\" data-ret-resume-book=\"{}\" href=\"{target}\">\
             <span class=\"me-now-item__copy\">\
             <strong class=\"me-now-item__title\">{book_title}</strong>\
             <span class=\"me-now-item__text\">Глава {}. {chapter_title} · {status}</span>\
             </span><span class=\"me-now-item__arrow\" aria-hidden=\"true\">→</span></a>",
            item.book_id, item.chapter_no,
        ));
    }
    // This is not a paid cursor, a payment grant or a publication pointer.
    // The archive route performs its own live entitlement check on every GET.
    for book in paid_only.iter().take(2) {
        let title = escape_html(&book.title);
        let state = if book.ordinary_deleted {
            "Удалена · Купленные главы"
        } else if book.pending_deletion {
            "Ожидает удаления · Купленные главы"
        } else {
            "Купленные главы"
        };
        links.push_str(&format!(
            "<a class=\"me-now-item me-now-item--reading\" data-ret-purchased-book=\"{}\" href=\"/me/library/{}/saved\">\
             <span class=\"me-now-item__copy\">\
             <strong class=\"me-now-item__title\">{title}</strong>\
             <span class=\"me-now-item__text\">{state} · Открыть закреплённые редакции</span>\
             </span><span class=\"me-now-item__arrow\" aria-hidden=\"true\">→</span></a>",
            book.book_id, book.book_id,
        ));
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ret03_home_and_library_share_safe_book_scoped_resume_links() {
        use tor_reading::services::progress::ContinueReading;
        let books = vec![
            ContinueReading {
                book_id: 7,
                book_title: "<img src=x onerror=alert(1)>".into(),
                chapter_id: 70,
                chapter_no: 3,
                chapter_title: "<script>bad</script>".into(),
                block_index: 2,
                archived_saved: false,
            },
            ContinueReading {
                book_id: 8,
                book_title: "Удалённая книга".into(),
                chapter_id: 80,
                chapter_no: 1,
                chapter_title: "Архив".into(),
                block_index: 1,
                archived_saved: true,
            },
            ContinueReading {
                book_id: 9,
                book_title: "No third Home card".into(),
                chapter_id: 90,
                chapter_no: 1,
                chapter_title: "Extra".into(),
                block_index: 0,
                archived_saved: false,
            },
        ];
        let cards = account_resume_cards(&books, &[]);
        assert_eq!(cards.matches("data-ret-resume-book=").count(), 2);
        assert!(cards.contains("href=\"/read/70#p-2\""));
        assert!(cards.contains("href=\"/me/library/saved/80#saved-resume\""));
        assert!(cards.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(cards.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!cards.contains("<script>bad</script>"));
        assert!(!cards.contains("No third Home card"));
        assert_eq!(
            super::super::super::library::reading_destination(&ContinueReading {
                block_index: 0,
                ..books[0].clone()
            }),
            "/read/70",
            "edited editions never inherit a stale fragment"
        );
        assert_eq!(
            super::super::super::library::reading_destination(&ContinueReading {
                block_index: 0,
                ..books[1].clone()
            }),
            "/me/library/saved/80",
            "archived V1 never redirects to a public chapter"
        );
    }
    #[test]
    fn ret03_paid_only_home_uses_private_archive_not_purchase_id() {
        use tor_reading::services::library::PurchasedOnlyBook;
        let purchased = vec![
            PurchasedOnlyBook {
                book_id: 51,
                title: "<img src=x onerror=alert(1)>".into(),
                pending_deletion: false,
                ordinary_deleted: true,
            },
            PurchasedOnlyBook {
                book_id: 52,
                title: "Вторая купленная".into(),
                pending_deletion: true,
                ordinary_deleted: false,
            },
            PurchasedOnlyBook {
                book_id: 53,
                title: "Third purchase".into(),
                pending_deletion: false,
                ordinary_deleted: false,
            },
        ];
        let html = account_resume_cards(&[], &purchased);
        assert_eq!(html.matches("data-ret-purchased-book=").count(), 2);
        assert!(html.contains("href=\"/me/library/51/saved\""));
        assert!(html.contains("href=\"/me/library/52/saved\""));
        assert!(html.contains("Удалена · Купленные главы"));
        assert!(html.contains("Ожидает удаления · Купленные главы"));
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(!html.contains("<img src=x"));
        assert!(!html.contains("Third purchase"));
        assert!(!html.contains("/read/paid/"));
        assert!(!html.contains("purchase_id="));
        assert!(account_resume_cards(&[], &[]).is_empty());
    }

}
