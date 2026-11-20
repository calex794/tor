//! RET-03c: paid-only Book Home shortcut verified through actual purchase + session.
//! This is owner-scoped navigation, never a substitute for a signed S3 grant.
use axum::{http::StatusCode, Router};

pub(super) async fn verify(
    app: &Router,
    url: &str,
    buyer: i64,
    book: i64,
    buyer_cookie: &str,
    stranger_cookie: &str,
) {
    assert_eq!(
        super::scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_library_items WHERE user_id={buyer} AND book_id={book}"
            ),
        ),
        0,
        "fixture must be an unbookmarked paid-only Book"
    );
    let response = super::private_get(app, "/me", Some(buyer_cookie)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = super::html_body(response).await;
    assert!(
        html.contains(&format!(
            "data-ret-purchased-book=\"{book}\" href=\"/me/library/{book}/saved\""
        )),
        "Home must link to the D-authorized private archive, not mint a paid grant"
    );
    assert!(!html.contains("purchase_id="));
    let stranger = super::private_get(app, "/me", Some(stranger_cookie)).await;
    assert_eq!(stranger.status(), StatusCode::OK);
    assert!(
        !super::html_body(stranger)
            .await
            .contains(&format!("data-ret-purchased-book=\"{book}\"")),
        "another reader cannot inherit a purchased-only Book shortcut"
    );

    // RET-02: the same buyer and exact purchase must agree across Home,
    // purchased Library and Reader while the owner book is on admin hold.
    // This is a temporary mutation of the disposable tor_ci test fixture.
    let chapter = super::scalar(
        url,
        &format!(
            "SELECT MIN(p.chapter_id) FROM chapter_purchase_facts p
             JOIN chapters c ON c.id=p.chapter_id
             WHERE p.user_id={buyer} AND c.book_id={book}"
        ),
    );
    let purchase = super::scalar(
        url,
        &format!(
            "SELECT MIN(id) FROM chapter_purchase_facts
             WHERE user_id={buyer} AND chapter_id={chapter}"
        ),
    );
    let count_sql = format!(
        "SELECT count(*) FROM chapter_purchase_facts p
         JOIN chapters c ON c.id=p.chapter_id
         WHERE p.user_id={buyer} AND c.book_id={book}"
    );
    let purchased_before = super::scalar(url, &count_sql);
    assert!(purchased_before > 0 && purchase > 0);
    assert_eq!(
        super::paid_page(app, chapter, purchase, buyer_cookie)
            .await
            .status(),
        StatusCode::OK
    );
    super::pg(url, &format!("UPDATE books SET admin_hold=true WHERE id={book}"));
    let held_home = super::private_get(app, "/me", Some(buyer_cookie)).await;
    assert_eq!(held_home.status(), StatusCode::OK);
    assert!(
        !super::html_body(held_home)
            .await
            .contains(&format!("data-ret-purchased-book=\"{book}\"")),
        "admin-held Book must not advertise a paid Home shortcut"
    );
    let held_library =
        super::private_get(app, "/me/library?view=purchased", Some(buyer_cookie)).await;
    assert_eq!(held_library.status(), StatusCode::OK);
    assert!(
        !super::html_body(held_library)
            .await
            .contains(&format!("href=\"/me/library/{book}/saved\"")),
        "admin-held Book must not expose a purchased Library archive link"
    );
    assert_eq!(
        super::paid_page(app, chapter, purchase, buyer_cookie)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "admin hold must deny the exact purchased Reader"
    );
    super::pg(url, &format!("UPDATE books SET admin_hold=false WHERE id={book}"));
    let restored_home = super::private_get(app, "/me", Some(buyer_cookie)).await;
    assert_eq!(restored_home.status(), StatusCode::OK);
    assert!(
        super::html_body(restored_home)
            .await
            .contains(&format!("data-ret-purchased-book=\"{book}\"")),
        "removing admin hold restores the authorized Home shortcut"
    );
    assert_eq!(
        super::paid_page(app, chapter, purchase, buyer_cookie)
            .await
            .status(),
        StatusCode::OK,
        "removing admin hold restores access to the same V1"
    );
    assert_eq!(
        super::scalar(url, &count_sql),
        purchased_before,
        "an admin hold must neither destroy nor fabricate purchase facts"
    );
}
