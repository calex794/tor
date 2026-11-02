//! C-12: NEwBOO administrator reads content without a buyer entitlement.
use super::{account, pg, read_page, scalar, BODY};
use axum::{
    body::to_bytes,
    http::{header, StatusCode},
    Router,
};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};

struct RoleCleanup<'a> {
    url: &'a str,
    actor: i64,
}
impl Drop for RoleCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE account_roles SET is_active=FALSE \
                 WHERE user_id={} AND role='administrator'",
                self.actor
            ),
        );
    }
}

pub(super) async fn assert_administrator_can_read(
    url: &str,
    accounts: &PgAccounts,
    browser: &Router,
    chapter: i64,
    version: i64,
    buyer: &str,
    author: &str,
) {
    let (admin, session) = account(accounts, "s065-real-admin@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({admin},'administrator',TRUE) \
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let _cleanup = RoleCleanup { url, actor: admin };
    let verified = accounts
        .session_actor(&session_token_digest(&session))
        .await
        .unwrap()
        .expect("admin session");
    assert!(verified.administrator);
    let path = format!("/admin/reader/chapters/{chapter}?version_id={version}");

    for (label, cookie, expected) in [
        ("guest", None, StatusCode::UNAUTHORIZED),
        (
            "buyer without administrator role",
            Some(buyer),
            StatusCode::FORBIDDEN,
        ),
        (
            "book author without administrator role",
            Some(author),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = read_page(browser, &path, cookie).await;
        assert_eq!(response.status(), expected, "{label}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(to_bytes(response.into_body(), 8192)
            .await
            .unwrap()
            .is_empty());
    }

    let response = read_page(browser, &path, Some(&session)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    assert!(response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'none'"));
    let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains(BODY),
        "administrator must see paid text without purchase"
    );
    assert!(html.contains("data-administrator-reader"));
    assert!(!html.contains("purchase_id="));
    assert!(!html.contains("dek_b64u"));

    // The administrator may view a book under moderation hold.
    let book = scalar(
        url,
        &format!("SELECT book_id FROM chapters WHERE id={chapter}"),
    );
    pg(
        url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
    );
    let held = read_page(browser, &path, Some(&session)).await;
    assert_eq!(held.status(), StatusCode::OK);
    assert!(String::from_utf8(
        to_bytes(held.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec()
    )
    .unwrap()
    .contains(BODY));
    pg(
        url,
        &format!("UPDATE books SET admin_hold=FALSE WHERE id={book}"),
    );

    // An arbitrary version_id can never escape the requested chapter.
    let missing = read_page(
        browser,
        &format!("/admin/reader/chapters/{chapter}?version_id=9223372036854775807"),
        Some(&session),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // Reading is not a financial event or a synthetic purchase.
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={admin}")
        ),
        0
    );

    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE \
             WHERE user_id={admin} AND role='administrator'"
        ),
    );
    let revoked = read_page(browser, &path, Some(&session)).await;
    assert_eq!(revoked.status(), StatusCode::FORBIDDEN);
    assert_eq!(revoked.headers()[header::CACHE_CONTROL], "no-store");
    assert!(to_bytes(revoked.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());
}
