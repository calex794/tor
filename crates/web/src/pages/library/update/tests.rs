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
    let html = update_cards(&page, false, false, &[], "csrf-token", None);
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
    let enabled = update_cards(&page, true, true, &[], "csrf-token", None);
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
    let disabled = update_cards(&page, false, true, &[], "csrf-token", None);
    assert!(!disabled.contains("/me/books/9/buy-chapters"));
    assert!(disabled.contains("/reader/12"));
    let owned = [tor_commerce::services::reader_views::ReadablePurchaseLink {
        chapter_id: 12,
        purchase_id: 77,
    }];
    let purchased = update_cards(&page, true, true, &owned, "csrf-token", None);
    assert!(purchased.contains("/read/paid/12?purchase_id=77"));
    assert!(!purchased.contains("/me/books/9/buy-chapters"));
    let unavailable = update_cards(&page, true, false, &owned, "csrf-token", None);
    assert!(unavailable.contains("Чтение временно недоступно"));
    assert!(!unavailable.contains("/read/paid/12"));
    assert!(!unavailable.contains("/me/books/9/buy-chapters"));
    let mut acknowledged = page.clone();
    acknowledged.items[0].is_read = true;
    let seen = update_cards(&acknowledged, true, true, &owned, "csrf-token", None);
    assert!(seen.contains("value=\"unread\""));
    assert!(seen.contains("Просмотрено"));
    assert!(seen.contains("Отметить непрочитанной"));
}

#[test]
fn ret01b_revision_card_uses_per_event_ack_and_safe_private_reader() {
    use tor_social::repositories::notifications::UserNotification;
    let records = [
        UserNotification {
            id: 91,
            kind: "chapter_updated".into(),
            title: "Глава <V2>".into(),
            body: "Автор обновил & текст".into(),
            url: "/read/12".into(),
            created_at: "18.11.2026 12:00".into(),
            is_read: false,
        },
        UserNotification {
            id: 92,
            kind: "chapter_updated".into(),
            title: "Bad link".into(),
            body: "".into(),
            url: "//evil.invalid".into(),
            created_at: "".into(),
            is_read: false,
        },
    ];
    let html = library_revision_cards(&records, "csrf-token");
    assert!(html.contains("Обновились"));
    assert!(html.contains("action=\"/notifications/91/read\""));
    assert!(html.contains("name=\"next\" value=\"/read/12\""));
    assert!(!html.contains("action=\"/me/updates/12/read-state\""));
    assert!(!html.contains("//evil.invalid"));
    assert!(html.contains("Глава &lt;V2&gt;"));
    assert!(html.contains("Автор обновил &amp; текст"));
    assert!(!html.contains("Глава <V2>"));
    assert!(html.contains("value=\"csrf-token\""));
}
