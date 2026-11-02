//! C-12 / S065: the live site-administrator role grants a PRIVATE reader,
//! not a fabricated purchase or a finance capability. Disposable tor_ci only.
use super::{account, pg, read_page, route, send, scalar, FakeG, ASSET_UUID, BODY, ORIGIN};
use axum::{
    body::to_bytes,
    http::{header, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};

struct AdminRoleCleanup<'a> {
    url: &'a str,
    user_id: i64,
}

impl Drop for AdminRoleCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE account_roles SET is_active=FALSE \\
                 WHERE user_id={} AND role='administrator'",
                self.user_id
            ),
        );
    }
}

struct ChapterStateCleanup<'a> {
    url: &'a str,
    book: i64,
    chapter: i64,
}

impl Drop for ChapterStateCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE books SET visibility='public',admin_hold=FALSE WHERE id={}; \\
                 UPDATE chapters SET visibility='public',admin_hold=FALSE WHERE id={}",
                self.book, self.chapter
            ),
        );
    }
}

pub(super) async fn assert_administrator_can_read_without_purchase(
    url: &str,
    accounts: &PgAccounts,
    reader: &Router,
    grants: &Router,
    fake: &Arc<FakeG>,
    edition: (i64, i64, i64, i64),
) {
    let (book, chapter, version, purchase) = edition;
    let (admin_id, admin_session) = account(accounts, "s065-c12-admin@tor-ci.example").await;
    let (_, outsider) = account(accounts, "s065-c12-no-role@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \\
             VALUES({admin_id},'administrator',TRUE) \\
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let _cleanup = AdminRoleCleanup {
        url,
        user_id: admin_id,
    };
    let actor = accounts
        .session_actor(&session_token_digest(&admin_session))
        .await
        .unwrap()
        .expect("administrator must have a real live session");
    assert!(actor.administrator);
    let csrf = generate_session_token();
    let path = route(chapter);
    let asset_path =
        format!("/api/v1/reader/chapters/{chapter}/assets/{ASSET_UUID}/delivery-grants");
    let page = format!("/read/admin/{chapter}");
    let normal = format!("/read/paid/{chapter}?purchase_id={purchase}");
    let old_preview =
        format!("/reader/{chapter}?preview=1&version_id={version}&purchase_id={purchase}");

    // The administrator's inspection is isolated from purchase APIs; old
    // direct buyer endpoints still refuse someone else's purchase identity.
    assert_eq!(
        read_page(reader, &normal, Some(&admin_session))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for (label, session, token, denial) in [
        ("guest", None, None, StatusCode::UNAUTHORIZED),
        ("user without administrator role", Some(outsider.as_str()), Some(csrf.as_str()), StatusCode::FORBIDDEN),
    ] {
        assert_eq!(read_page(reader, &page, session).await.status(), denial, "{label}");
        for target in [&path, &asset_path] {
            let r = send(
                grants,
                target,
                session,
                token,
                Some(ORIGIN),
                r#"{"source":"administrator"}"#,
            )
            .await;
            assert_eq!(r.status(), denial, "{label}: {target}");
            assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
            assert!(to_bytes(r.into_body(), 8192).await.unwrap().is_empty());
        }
    }
    let preview = read_page(reader, &old_preview, Some(&admin_session)).await;
    assert_eq!(preview.status(), StatusCode::OK);
    let body = to_bytes(preview.into_body(), 256 * 1024).await.unwrap();
    assert!(!body.windows(BODY.len()).any(|part| part == BODY.as_bytes()));

    // Only the private admin shell can start the authenticated WebCrypto flow.
    let shell = read_page(reader, &page, Some(&admin_session)).await;
    assert_eq!(shell.status(), StatusCode::OK);
    assert_eq!(shell.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(shell.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(shell.headers()["x-robots-tag"], "noindex");
    let html = String::from_utf8(to_bytes(shell.into_body(), 128 * 1024).await.unwrap().to_vec())
        .unwrap();
    assert!(html.contains("data-admin-chapter-reader"));
    assert!(html.contains("newboo-admin-reader.mjs"));
    assert!(!html.contains(BODY));
    assert!(!html.contains("dek_b64u"));
    assert!(!html.contains("ciphertext_url"));
    assert!(!html.contains("data-purchase-id"));

    assert_eq!(
        send(
            grants,
            &path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            &json!({"source":"administrator","purchase_id":purchase.to_string()}).to_string(),
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an admin cannot turn a stolen purchase_id into an administrative credential"
    );
    let grant = send(
        grants,
        &path,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        r#"{"source":"administrator"}"#,
    )
    .await;
    assert_eq!(grant.status(), StatusCode::OK);
    assert_eq!(grant.headers()[header::CACHE_CONTROL], "private, no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(grant.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["version_id"], version.to_string());
    assert!(!body.to_string().contains(BODY));

    let image = send(
        grants,
        &asset_path,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        r#"{"source":"administrator"}"#,
    )
    .await;
    assert_eq!(image.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(image.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(value["version_id"], version.to_string());
    assert_eq!(value["object_id"], ASSET_UUID);
    assert_eq!(fake.signed.load(Ordering::SeqCst), 2);
    assert_eq!(
        scalar(url, &format!("SELECT count(*) FROM chapter_purchase_facts WHERE user_id={admin_id}")),
        0
    );
    assert_eq!(
        scalar(url, &format!("SELECT count(*) FROM reader_wallet_entries WHERE user_id={admin_id}")),
        0
    );

    // Hidden/draft/held content remains visible only to the privileged
    // reader while an immutable B/G sealed edition still exists.
    let _restore = ChapterStateCleanup { url, book, chapter };
    pg(
        url,
        &format!(
            "UPDATE books SET visibility='draft',admin_hold=TRUE WHERE id={book}; \\
             UPDATE chapters SET visibility='draft',admin_hold=TRUE WHERE id={chapter}"
        ),
    );
    let held = send(
        grants,
        &path,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        r#"{"source":"administrator"}"#,
    )
    .await;
    assert_eq!(held.status(), StatusCode::OK);
    let held_body: Value =
        serde_json::from_slice(&to_bytes(held.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(held_body["version_id"], version.to_string());
    assert_eq!(fake.signed.load(Ordering::SeqCst), 3);

    // Revocation must invalidate the next admin GET/POST, including a held
    // chapter; no new S3 bearer or a decrypted edition may be delivered.
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE \\
             WHERE user_id={admin_id} AND role='administrator'"
        ),
    );
    assert_eq!(
        read_page(reader, &page, Some(&admin_session))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for target in [&path, &asset_path] {
        let r = send(
            grants,
            target,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            r#"{"source":"administrator"}"#,
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
        assert!(to_bytes(r.into_body(), 8192).await.unwrap().is_empty());
    }
    assert_eq!(fake.signed.load(Ordering::SeqCst), 3);
    // Historic NEwBOO aliases are absent, not unguarded admin back doors.
    for path in [
        format!("/reader/{chapter}/history?version_id={version}"),
        format!("/api/v1/chapters/{chapter}/versions/{version}"),
        format!("/cdn/chapters/{chapter}/{version}.html"),
    ] {
        assert_eq!(
            read_page(reader, &path, Some(&admin_session))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    // Administrator inspection is not permission to read the buyer's
    // progress marker.
    let progress = format!(
        "/api/v1/me/progress/purchased?purchase_id={purchase}&chapter_id={chapter}&version_id={version}"
    );
    assert_eq!(
        read_page(reader, &progress, Some(&admin_session))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}
