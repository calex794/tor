//! C-12/S065: a live site administrator may inspect an unsealed working
//! chapter without a purchase, even when the book or chapter is unpublished.
//! The ordinary reader and paid S3 grant must not inherit this authority.
use super::{account, pg, read_page, scalar, send, FakeG, ORIGIN};
use axum::{
    body::to_bytes,
    http::{header, StatusCode},
    Router,
};
use serde_json::Value;
use std::sync::{atomic::Ordering, Arc};
use tor_accounts::repositories::PgAccounts;

/// Full authenticated PostgreSQL web router with no paid G adapter,
/// private S3 origin or purchased-content browser.
pub(super) fn pg_only_app(
    catalog: tor_books::repositories::PgCatalog,
    reader: tor_reading::repositories::PgReader,
    accounts: PgAccounts,
) -> Router {
    tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.to_owned())
}

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

struct ModeratorRoleCleanup<'a> {
    url: &'a str,
    user_id: i64,
}
impl Drop for ModeratorRoleCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE account_roles SET is_active=FALSE \
                 WHERE user_id={} AND role='moderator'",
                self.user_id
            ),
        );
    }
}

pub(super) async fn assert_unsealed_admin_read(
    url: &str,
    accounts: &PgAccounts,
    app: &Router,
    api_only: &Router,
    no_storage: &Router,
    fake: &Arc<FakeG>,
    book: i64,
) {
    let (admin_id, admin_session) = account(accounts, "s065-admin-unsealed@tor-ci.example").await;
    let (moderator_id, outsider) = account(accounts, "s065-unsealed-outsider@tor-ci.example").await;
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
    // A live moderator is still not a site administrator. The resulting
    // denials cover both free and paid unpublished working drafts in each
    // authenticated router mode (browser, API-only G, no-G PG).
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({moderator_id},'moderator',TRUE) \
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let _moderator_cleanup = ModeratorRoleCleanup {
        url,
        user_id: moderator_id,
    };
    let mod_actor = accounts
        .session_actor(&tor_accounts::services::security::session_token_digest(
            &outsider,
        ))
        .await
        .unwrap()
        .unwrap();
    assert!(mod_actor.moderator && !mod_actor.administrator);
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
        // Without S3 browser mode, only this unsealed admin inspection is
        // available. The paid reader stays absent and no S3 origin enters CSP.
        let no_s3_shell = read_page(api_only, &shell_path, Some(&admin_session)).await;
        assert_eq!(no_s3_shell.status(), StatusCode::OK);
        assert_eq!(no_s3_shell.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            no_s3_shell.headers()[header::REFERRER_POLICY],
            "no-referrer"
        );
        assert_eq!(
            no_s3_shell.headers()["content-security-policy"],
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
        );
        let no_s3_html = to_bytes(no_s3_shell.into_body(), 128 * 1024).await.unwrap();
        let no_s3_html = String::from_utf8(no_s3_html.to_vec()).unwrap();
        assert!(no_s3_html.contains("data-admin-unsealed=\"true\""));
        assert!(no_s3_html.contains("data-s3-origin=\"\""));
        assert!(!no_s3_html.contains(body));
        assert!(!no_s3_html.contains("ciphertext_url"));
        // This is the actual default authenticated PostgreSQL router: no
        // paid S3 bucket, KEK, private G port or paid grant routes exist.
        let pg_only_shell = read_page(no_storage, &shell_path, Some(&admin_session)).await;
        assert_eq!(pg_only_shell.status(), StatusCode::OK);
        assert_eq!(pg_only_shell.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            pg_only_shell.headers()["content-security-policy"],
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
        );
        let pg_only_html = to_bytes(pg_only_shell.into_body(), 128 * 1024)
            .await
            .unwrap();
        let pg_only_html = String::from_utf8(pg_only_html.to_vec()).unwrap();
        assert!(pg_only_html.contains("data-admin-unsealed=\"true\""));
        assert!(pg_only_html.contains("data-s3-origin=\"\""));
        assert!(!pg_only_html.contains(body));
        assert_eq!(
            read_page(
                api_only,
                &format!("/read/paid/{chapter}"),
                Some(&admin_session)
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        // The canonical NEwBOO chapter URL must lead the site
        // administrator to the existing private unsealed reader, without
        // embedding content, a version ID or any signed URL in the redirect.
        let canonical = format!("/reader/{chapter}");
        let pg_redirect = read_page(no_storage, &canonical, Some(&admin_session)).await;
        assert_eq!(pg_redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            pg_redirect.headers()[header::LOCATION],
            format!("/read/admin/{chapter}")
        );
        let api_redirect = read_page(api_only, &canonical, Some(&admin_session)).await;
        assert_eq!(api_redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            api_redirect.headers()[header::LOCATION],
            format!("/read/admin/{chapter}")
        );
        assert_eq!(api_redirect.headers()[header::CACHE_CONTROL], "no-store");
        let redirect = read_page(app, &canonical, Some(&admin_session)).await;
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            redirect.headers()[header::LOCATION],
            format!("/read/admin/{chapter}")
        );
        assert_eq!(redirect.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(redirect.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(redirect.headers()["x-robots-tag"], "noindex, nofollow");
        let bytes = to_bytes(redirect.into_body(), 8192).await.unwrap();
        assert!(!bytes
            .windows(body.len())
            .any(|window| window == body.as_bytes()));
        assert!(!bytes
            .windows(b"dek_b64u".len())
            .any(|window| window == b"dek_b64u"));
        for session in [None, Some(outsider.as_str())] {
            let denied = read_page(app, &canonical, session).await;
            assert_eq!(denied.status(), StatusCode::NOT_FOUND);
            assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
            let bytes = to_bytes(denied.into_body(), 8192).await.unwrap();
            assert!(!bytes
                .windows(body.len())
                .any(|window| window == body.as_bytes()));
            assert_eq!(
                read_page(api_only, &canonical, session).await.status(),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                read_page(no_storage, &canonical, session).await.status(),
                StatusCode::NOT_FOUND
            );
        }
        for (session, token, origin, expected) in [
            (None, None, Some(ORIGIN), StatusCode::UNAUTHORIZED),
            (
                Some(outsider.as_str()),
                Some(csrf.as_str()),
                Some(ORIGIN),
                StatusCode::FORBIDDEN,
            ),
            (
                Some(admin_session.as_str()),
                None,
                Some(ORIGIN),
                StatusCode::FORBIDDEN,
            ),
            (
                Some(admin_session.as_str()),
                Some(csrf.as_str()),
                Some("https://evil.invalid"),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let denied = send(app, &content_path, session, token, origin, "{}").await;
            assert_eq!(denied.status(), expected);
            assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
            assert!(to_bytes(denied.into_body(), 8192).await.unwrap().is_empty());
            assert_eq!(
                send(api_only, &content_path, session, token, origin, "{}")
                    .await
                    .status(),
                expected
            );
            assert_eq!(
                send(no_storage, &content_path, session, token, origin, "{}")
                    .await
                    .status(),
                expected
            );
        }
        let no_s3_permitted = send(
            api_only,
            &content_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(no_s3_permitted.status(), StatusCode::OK);
        assert_eq!(no_s3_permitted.headers()[header::CACHE_CONTROL], "no-store");
        let no_s3_payload: Value =
            serde_json::from_slice(&to_bytes(no_s3_permitted.into_body(), 8192).await.unwrap())
                .unwrap();
        assert_eq!(no_s3_payload["version_id"], version.to_string());
        assert_eq!(no_s3_payload["body_markdown"], body);
        let pg_only_permitted = send(
            no_storage,
            &content_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(pg_only_permitted.status(), StatusCode::OK);
        let pg_payload: Value =
            serde_json::from_slice(&to_bytes(pg_only_permitted.into_body(), 8192).await.unwrap())
                .unwrap();
        assert_eq!(pg_payload["version_id"], version.to_string());
        assert_eq!(pg_payload["body_markdown"], body);
        // No private paid grant can be synthesized in the no-G router.
        assert_eq!(
            send(
                no_storage,
                &format!("/api/v1/reader/chapters/{chapter}/delivery-grants"),
                Some(&admin_session),
                Some(&csrf),
                Some(ORIGIN),
                r#"{"source":"administrator"}"#,
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
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
        let payload: Value =
            serde_json::from_slice(&to_bytes(permitted.into_body(), 8192).await.unwrap()).unwrap();
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
        let payload: Value =
            serde_json::from_slice(&to_bytes(switched.into_body(), 8192).await.unwrap()).unwrap();
        assert_eq!(payload["version_id"], next.to_string());
        assert_eq!(payload["body_markdown"], "NEW PRIVATE REVISION");
        assert!(!payload.to_string().contains(body));
        let api_switched = send(
            api_only,
            &content_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(api_switched.status(), StatusCode::OK);
        let payload: Value =
            serde_json::from_slice(&to_bytes(api_switched.into_body(), 8192).await.unwrap())
                .unwrap();
        assert_eq!(payload["version_id"], next.to_string());
        assert_eq!(payload["body_markdown"], "NEW PRIVATE REVISION");
        let switched_pg = send(
            no_storage,
            &content_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(switched_pg.status(), StatusCode::OK);
        let current: Value =
            serde_json::from_slice(&to_bytes(switched_pg.into_body(), 8192).await.unwrap())
                .unwrap();
        assert_eq!(current["version_id"], next.to_string());
        assert_eq!(current["body_markdown"], "NEW PRIVATE REVISION");
        // Historical text is reachable only through the separate audited
        // admin POST, not via the current reader or forged query parameters.
        let history_path = format!("/api/v1/reader/chapters/{chapter}/admin-history");
        let old_path = format!("{history_path}/{version}");
        let history = send(
            app, &history_path, Some(&admin_session), Some(&csrf), Some(ORIGIN), "{}",
        ).await;
        assert_eq!(history.status(), StatusCode::OK);
        assert_eq!(history.headers()[header::CACHE_CONTROL], "no-store");
        let versions: Value = serde_json::from_slice(
            &to_bytes(history.into_body(), 8192).await.unwrap(),
        ).unwrap();
        assert_eq!(versions["schema"], "tor.admin_history.v1");
        assert_eq!(versions["versions"].as_array().unwrap().len(), 2);
        assert!(versions["versions"].as_array().unwrap().iter()
            .any(|entry| entry["version_id"].as_str() == Some(version.to_string().as_str())));
        for (session, csrf, origin, expected) in [
            (None, None, Some(ORIGIN), StatusCode::UNAUTHORIZED),
            (
                Some(outsider.as_str()),
                Some(csrf.as_str()),
                Some(ORIGIN),
                StatusCode::FORBIDDEN,
            ),
            (
                Some(admin_session.as_str()),
                None,
                Some(ORIGIN),
                StatusCode::FORBIDDEN,
            ),
            (
                Some(admin_session.as_str()),
                Some(csrf.as_str()),
                Some("https://evil.invalid"),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let response = send(app, &old_path, session, csrf, origin, "{}").await;
            assert_eq!(response.status(), expected);
            assert!(to_bytes(response.into_body(), 8192)
                .await
                .unwrap()
                .is_empty());
        }
        let historical = send(
            app,
            &old_path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(historical.status(), StatusCode::OK);
        assert_eq!(historical.headers()[header::REFERRER_POLICY], "no-referrer");
        let historical: Value =
            serde_json::from_slice(&to_bytes(historical.into_body(), 8192).await.unwrap()).unwrap();
        assert_eq!(historical["version_id"], version.to_string());
        assert_eq!(historical["body_markdown"], body);
        // A version from another chapter or an unbound random identifier is
        // never a valid historical entitlement for this chapter.
        let foreign = send(
            app,
            &format!("{history_path}/999999999"),
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            "{}",
        )
        .await;
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        assert!(to_bytes(foreign.into_body(), 8192)
            .await
            .unwrap()
            .is_empty());
        assert!(!versions.to_string().contains(body));
    }
    // Two versions of each free/paid draft were delivered through each
    // of the three supported admin router modes, never to other actors.
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_admin_chapter_access_audit \
                 WHERE admin_user_id={admin_id} AND action='unsealed_body'"
            )
        ),
        14
    );
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
    assert_eq!(
        read_page(app, &format!("/reader/{chapter}"), Some(&admin_session))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "revoked administrators must not navigate to the private reader"
    );
    assert_eq!(
        read_page(
            api_only,
            &format!("/read/admin/{chapter}"),
            Some(&admin_session)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        read_page(
            api_only,
            &format!("/reader/{chapter}"),
            Some(&admin_session)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
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
    assert!(to_bytes(revoked.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        read_page(
            no_storage,
            &format!("/read/admin/{chapter}"),
            Some(&admin_session)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        read_page(
            no_storage,
            &format!("/reader/{chapter}"),
            Some(&admin_session)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let revoked_pg = send(
        no_storage,
        &format!("/api/v1/reader/chapters/{chapter}/admin-unsealed"),
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(revoked_pg.status(), StatusCode::FORBIDDEN);
    assert!(to_bytes(revoked_pg.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());
    let revoked_no_s3 = send(
        api_only,
        &format!("/api/v1/reader/chapters/{chapter}/admin-unsealed"),
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(revoked_no_s3.status(), StatusCode::FORBIDDEN);
    let revoked_history = send(
        app,
        &format!("/api/v1/reader/chapters/{chapter}/admin-history"),
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(revoked_history.status(), StatusCode::FORBIDDEN);
    assert!(to_bytes(revoked_history.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());

    assert!(to_bytes(revoked_no_s3.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_admin_chapter_access_audit \
                 WHERE admin_user_id={admin_id}"
            )
        ),
        14,
        "revoked or denied reads cannot create successful-delivery audit records"
    );
}
