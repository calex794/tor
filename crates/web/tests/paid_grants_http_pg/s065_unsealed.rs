//! C-12/S065: a live site administrator may inspect an unsealed working
//! chapter without a purchase, even when the book or chapter is unpublished.
//! The ordinary reader and paid S3 grant must not inherit this authority.
use super::super::{account, pg, read_page, scalar, send, FakeG, ORIGIN};
use axum::{
    body::to_bytes,
    http::{header, StatusCode},
    Router,
};
use serde_json::Value;
use std::sync::{atomic::Ordering, Arc};
use tor_accounts::repositories::PgAccounts;

struct AdminRoleCleanup<'a> {
    url: &'a str,
    user_id: i64,
}
impl Drop for AdminRoleCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE account_roles SET is_active=FALSE                  WHERE user_id={} AND role='administrator'",
                self.user_id
            ),
        );
    }
}

pub(super) async fn assert_unsealed_admin_read(
    url: &str,
    accounts: &PgAccounts,
    app: &Router,
    fake: &Arc<FakeG>,
    book: i64,
) {
    let (admin_id, admin_session) =
        account(accounts, "s065-admin-unsealed@tor-ci.example").await;
    let (_, outsider) = account(accounts, "s065-unsealed-outsider@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)              VALUES({admin_id},'administrator',TRUE)              ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let _cleanup = AdminRoleCleanup {
        url,
        user_id: admin_id,
    };
    let csrf = tor_accounts::services::security::generate_session_token();
    let signed_before = fake.signed.load(Ordering::SeqCst);
    // These two drafts have no published pointer or B/G-attached object.
    // Their body is never legal on a normal public/paywall or paid-grant path.
    for (number, access, price, body) in [
        (501, "free", 0, "S065-PRIVATE-FREE-DRAFT-NEVER-IN-SSR"),
        (502, "paid", 175, "S065-PRIVATE-PAID-DRAFT-NEVER-IN-SSR"),
    ] {
        let chapter = scalar(
            url,
            &format!(
                "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)                  VALUES({book},{number},'S065 draft','draft','{access}',{price}) RETURNING id"
            ),
        );
        let version = scalar(
            url,
            &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)                  VALUES({chapter},1,'{body}') RETURNING id"
            ),
        );
        pg(
            url,
            &format!(
                "INSERT INTO chapter_working_drafts(chapter_id,version_id)                  VALUES({chapter},{version})"
            ),
        );
        let shell_path = format!("/read/admin/{chapter}");
        let content_path = format!("/api/v1/reader/chapters/{chapter}/admin-unsealed");
        let guest = read_page(app, &shell_path, None).await;
        assert_eq!(guest.status(), StatusCode::UNAUTHORIZED);
        let stranger = read_page(app, &shell_path, Some(&outsider)).await;
        assert_eq!(stranger.status(), StatusCode::FORBIDDEN);
        let shell = read_page(app, &shell_path, Some(&admin_session)).await;
        assert_eq!(shell.status(), StatusCode::OK);
        assert_eq!(shell.headers()[header::CACHE_CONTROL], "no-store");
        let html = to_bytes(shell.into_body(), 128 * 1024).await.unwrap();
        let html = String::from_utf8(html.to_vec()).unwrap();
        assert!(html.contains("data-admin-unsealed=\"true\""));
        assert!(!html.contains(body));
        assert!(!html.contains("dek_b64u"));
        assert_eq!(
            read_page(app, &format!("/reader/{chapter}"), None)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        for (session, token, origin, expected) in [
            (None, None, Some(ORIGIN), StatusCode::UNAUTHORIZED),
            (Some(outsider.as_str()), Some(csrf.as_str()), Some(ORIGIN), StatusCode::FORBIDDEN),
            (Some(admin_session.as_str()), None, Some(ORIGIN), StatusCode::FORBIDDEN),
            (Some(admin_session.as_str()), Some(csrf.as_str()), Some("https://evil.invalid"), StatusCode::FORBIDDEN),
        ] {
            let denied = send(app, &content_path, session, token, origin, "{}").await;
            assert_eq!(denied.status(), expected);
            assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
            assert!(to_bytes(denied.into_body(), 8192).await.unwrap().is_empty());
        }
        let permitted = send(
            app,
            &content_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(permitted.status(), StatusCode::OK);
        assert_eq!(permitted.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(permitted.headers()[header::REFERRER_POLICY], "no-referrer");
        let payload: Value = serde_json::from_slice(
            &to_bytes(permitted.into_body(), 8192).await.unwrap(),
        )
        .unwrap();
        assert_eq!(payload["schema"], "tor.admin_unsealed.v1");
        assert_eq!(payload["chapter_id"], chapter.to_string());
        assert_eq!(payload["version_id"], version.to_string());
        assert_eq!(payload["body_markdown"], body);
        assert_eq!(fake.signed.load(Ordering::SeqCst), signed_before);
        // The new draft becomes current; old bodies must never be selectable
        // through a forged query or a guessed historical version ID.
        let next = scalar(
            url,
            &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)                  VALUES({chapter},2,'NEW PRIVATE REVISION') RETURNING id"
            ),
        );
        pg(
            url,
            &format!(
                "UPDATE chapter_working_drafts SET version_id={next}                  WHERE chapter_id={chapter}"
            ),
        );
        let switched = send(
            app,
            &content_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(switched.status(), StatusCode::OK);
        let payload: Value = serde_json::from_slice(
            &to_bytes(switched.into_body(), 8192).await.unwrap(),
        )
        .unwrap();
        assert_eq!(payload["version_id"], next.to_string());
        assert_eq!(payload["body_markdown"], "NEW PRIVATE REVISION");
        assert!(!payload.to_string().contains(body));
    }
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={admin_id}")
        ),
        0
    );
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM reader_wallet_entries WHERE user_id={admin_id}")
        ),
        0
    );
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE WHERE user_id={admin_id}              AND role='administrator'"
        ),
    );
    let chapter = scalar(
        url,
        &format!("SELECT id FROM chapters WHERE book_id={book} AND chapter_no=502"),
    );
    assert_eq!(
        read_page(app, &format!("/read/admin/{chapter}"), Some(&admin_session))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let revoked = send(
        app,
        &format!("/api/v1/reader/chapters/{chapter}/admin-unsealed"),
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::FORBIDDEN);
    assert!(to_bytes(revoked.into_body(), 8192).await.unwrap().is_empty());
}
