//! C-12 / S065: active site-admin role is not a paid entitlement.
//! Only disposable tor_ci; no production role grants.
use super::{account, pg, read_page, route, send, ASSET_UUID, BODY, ORIGIN};
use axum::{
    body::to_bytes,
    http::{header, StatusCode},
    Router,
};
use serde_json::json;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};

pub(super) async fn assert_administrator_and_guest_denied(
    url: &str,
    accounts: &PgAccounts,
    reader: &Router,
    grants: &Router,
    chapter: i64,
    version: i64,
    purchase: i64,
) {
    // A genuine A session-backed role, not a client-supplied role claim.
    let (admin_id, admin_session) =
        account(accounts, "s065-c12-admin@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({admin_id},'administrator',TRUE) \
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let actor = accounts
        .session_actor(&session_token_digest(&admin_session))
        .await
        .unwrap()
        .expect("site administrator must have a live session");
    assert!(actor.administrator, "fixture must exercise an active administrator");

    let csrf = generate_session_token();
    let payload = json!({ "purchase_id": purchase.to_string() }).to_string();
    let chapter_grant = route(chapter);
    let asset_grant =
        format!("/api/v1/reader/chapters/{chapter}/assets/{ASSET_UUID}/delivery-grants");
    let paid_page = format!("/read/paid/{chapter}?purchase_id={purchase}");
    let forged_preview =
        format!("/reader/{chapter}?preview=1&version_id={version}&purchase_id={purchase}");

    for (label, session, token, denial) in [
        ("guest", None, None, StatusCode::UNAUTHORIZED),
        (
            "administrator without purchase",
            Some(admin_session.as_str()),
            Some(csrf.as_str()),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let page = read_page(reader, &paid_page, session).await;
        assert_eq!(page.status(), denial, "{label}: paid reader");
        assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
        assert!(to_bytes(page.into_body(), 8192).await.unwrap().is_empty());

        for path in [&chapter_grant, &asset_grant] {
            let reply = send(grants, path, session, token, Some(ORIGIN), &payload).await;
            assert_eq!(reply.status(), denial, "{label}: {path}");
            assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
            assert!(to_bytes(reply.into_body(), 8192).await.unwrap().is_empty());
        }

        // Neither a forged query nor the administrator role unlocks the paywall.
        let preview = read_page(reader, &forged_preview, session).await;
        assert_eq!(preview.status(), StatusCode::OK, "{label}: paywall");
        assert_eq!(preview.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(preview.into_body(), 256 * 1024).await.unwrap();
        assert!(!body.windows(BODY.len()).any(|v| v == BODY.as_bytes()));
        assert!(!body
            .windows(b"X-Amz-Signature".len())
            .any(|v| v == b"X-Amz-Signature"));
        assert!(!String::from_utf8_lossy(&body).contains(&format!("purchase_id={purchase}")));

        // These historic NEwBOO routes are absent in TOR. A 404 here proves
        // a closed bypass path, not history/preview feature parity.
        for path in [
            format!("/reader/{chapter}/history?version_id={version}"),
            format!("/reader/{chapter}/preview?version_id={version}"),
            format!("/api/v1/chapters/{chapter}/versions/{version}"),
            format!("/cdn/chapters/{chapter}/{version}.html"),
            format!("/media/chapters/{chapter}/{version}"),
        ] {
            let reply = read_page(reader, &path, session).await;
            assert_eq!(reply.status(), StatusCode::NOT_FOUND, "{label}: {path}");
            let bytes = to_bytes(reply.into_body(), 8192).await.unwrap();
            assert!(!bytes.windows(BODY.len()).any(|v| v == BODY.as_bytes()));
            assert!(!bytes
                .windows(b"X-Amz-Signature".len())
                .any(|v| v == b"X-Amz-Signature"));
        }
    }

    // Owned progress is not a substitute entitlement for a site admin.
    let progress = format!(
        "/api/v1/me/progress/purchased?purchase_id={purchase}&chapter_id={chapter}&version_id={version}"
    );
    let response = read_page(reader, &progress, Some(&admin_session)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(to_bytes(response.into_body(), 8192).await.unwrap().is_empty());
}
