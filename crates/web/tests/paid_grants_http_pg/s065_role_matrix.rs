//! C-12 / S065: the live site-administrator role grants a PRIVATE reader,
//! not a fabricated purchase or a finance capability. Disposable tor_ci only.
use super::{account, pg, read_page, route, scalar, send, FakeG, ASSET_UUID, BODY, ORIGIN};
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
use tor_contracts::paid_delivery::PositiveId;

struct AdminRoleCleanup<'a> {
    url: &'a str,
    user_id: i64,
}

impl Drop for AdminRoleCleanup<'_> {
    fn drop(&mut self) {
        pg(
            self.url,
            &format!(
                "UPDATE account_roles SET is_active=FALSE \
                 WHERE user_id={} AND role='administrator'",
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
                "UPDATE books SET visibility='public',admin_hold=FALSE WHERE id={}; \
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
    storage_free: &Router,
    fake: &Arc<FakeG>,
    edition: (i64, i64, i64, i64),
) {
    let (book, chapter, version, purchase) = edition;
    let (admin_id, admin_session) = account(accounts, "s065-c12-admin@tor-ci.example").await;
    let (moderator_id, outsider) = account(accounts, "s065-c12-no-role@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) \
             VALUES({admin_id},'administrator',TRUE) \
             ON CONFLICT(user_id,role) DO UPDATE SET is_active=TRUE"
        ),
    );
    let _cleanup = AdminRoleCleanup {
        url,
        user_id: admin_id,
    };
    // Moderation is a real independent A role, not an admin/wallet grant.
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
        .session_actor(&session_token_digest(&outsider))
        .await
        .unwrap()
        .unwrap();
    assert!(mod_actor.moderator);
    assert!(!mod_actor.administrator);
    // Every outsider denial below now tests an *active moderator*, not just
    // an unprivileged registered user.
    let actor = accounts
        .session_actor(&session_token_digest(&admin_session))
        .await
        .unwrap()
        .expect("administrator must have a real live session");
    assert!(actor.administrator);
    // An exact historical sealed version is an A-scoped administrative
    // capability, not a purchase or a public browse parameter.
    let authority = tor_commerce::repositories::admin_reader_entitlements::
        PgAdministratorReaderEntitlements::new(
            accounts.pool().clone(),
            session_token_digest(&admin_session),
        );
    let admin_actor = tor_contracts::paid_delivery::Actor {
        user_id: PositiveId::new(admin_id).unwrap(),
    };
    let chapter_id = PositiveId::new(chapter).unwrap();
    let version_id = PositiveId::new(version).unwrap();
    assert_eq!(
        authority
            .sealed_historical_version(admin_actor, chapter_id, version_id)
            .await
            .unwrap(),
        Some(tor_contracts::paid_delivery::ChapterVersionRef {
            chapter_id,
            version_id,
        }),
    );
    assert!(authority
        .sealed_history(admin_actor, chapter_id)
        .await
        .unwrap()
        .iter()
        .any(|(id, _)| *id == version_id));
    assert_eq!(
        authority
            .sealed_historical_version(
                admin_actor,
                chapter_id,
                PositiveId::new(9_999_999_999).unwrap(),
            )
            .await
            .unwrap(),
        None,
    );
    let moderator_authority = tor_commerce::repositories::admin_reader_entitlements::
        PgAdministratorReaderEntitlements::new(
            accounts.pool().clone(),
            session_token_digest(&outsider),
        );
    assert_eq!(
        moderator_authority
            .sealed_historical_version(
                tor_contracts::paid_delivery::Actor {
                    user_id: PositiveId::new(moderator_id).unwrap(),
                },
                chapter_id,
                version_id,
            )
            .await
            .unwrap(),
        None,
    );
    let csrf = generate_session_token();
    let path = route(chapter);
    let asset_path =
        format!("/api/v1/reader/chapters/{chapter}/assets/{ASSET_UUID}/delivery-grants");
    let page = format!("/read/admin/{chapter}");
    let normal = format!("/read/paid/{chapter}?purchase_id={purchase}");
    // API-only mode must not manufacture an encrypted browser shell.
    assert_eq!(
        read_page(grants, &page, Some(&admin_session))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        read_page(grants, &normal, Some(&admin_session))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let old_preview =
        format!("/reader/{chapter}?preview=1&version_id={version}&purchase_id={purchase}");

    // A standard PG/auth server without any private G storage must not
    // redirect sealed paid content to a reader that cannot deliver it.
    for path in [&page, &normal] {
        assert_eq!(
            read_page(storage_free, path, Some(&admin_session))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    for path in [format!("/reader/{chapter}"), old_preview.clone()] {
        let paywall = read_page(storage_free, &path, Some(&admin_session)).await;
        assert_eq!(paywall.status(), StatusCode::OK);
        let html = to_bytes(paywall.into_body(), 256 * 1024).await.unwrap();
        assert!(!html
            .windows(BODY.len())
            .any(|bytes| bytes == BODY.as_bytes()));
    }
    assert_eq!(
        send(
            storage_free,
            &path,
            Some(&admin_session),
            Some(&csrf),
            Some(ORIGIN),
            r#"{"source":"administrator"}"#
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "PG-only reader must not expose any private paid G delivery grant"
    );

    // The administrator's inspection is isolated from purchase APIs; old
    // direct buyer endpoints still refuse someone else's purchase identity.
    assert_eq!(
        read_page(reader, &normal, Some(&admin_session))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        read_page(reader, &normal, Some(&outsider)).await.status(),
        StatusCode::FORBIDDEN,
        "moderator must not inherit another user's direct paid purchase"
    );
    for (label, session, token, denial) in [
        ("guest", None, None, StatusCode::UNAUTHORIZED),
        (
            "user without administrator role",
            Some(outsider.as_str()),
            Some(csrf.as_str()),
            StatusCode::FORBIDDEN,
        ),
    ] {
        assert_eq!(
            read_page(reader, &page, session).await.status(),
            denial,
            "{label}"
        );
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
    // The original NEwBOO reader link must lead an active administrator to
    // the private reader, regardless of a forged preview/purchase query.
    let canonical = format!("/reader/{chapter}");
    for target in [&canonical, &old_preview] {
        let redirect = read_page(reader, target, Some(&admin_session)).await;
        assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            redirect.headers()[header::LOCATION],
            format!("/read/admin/{chapter}")
        );
        assert_eq!(redirect.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(redirect.headers()[header::REFERRER_POLICY], "no-referrer");
        let body = to_bytes(redirect.into_body(), 8192).await.unwrap();
        assert!(!body.windows(BODY.len()).any(|part| part == BODY.as_bytes()));
        assert!(!body
            .windows(b"X-Amz-Signature".len())
            .any(|part| part == b"X-Amz-Signature"));
    }
    // An unrelated account still sees only the public paid metadata paywall.
    let outsider_page = read_page(reader, &old_preview, Some(&outsider)).await;
    assert_eq!(outsider_page.status(), StatusCode::OK);
    let body = to_bytes(outsider_page.into_body(), 256 * 1024)
        .await
        .unwrap();
    assert!(!body.windows(BODY.len()).any(|part| part == BODY.as_bytes()));

    // Only the private admin shell can start the authenticated WebCrypto flow.
    let shell = read_page(reader, &page, Some(&admin_session)).await;
    assert_eq!(shell.status(), StatusCode::OK);
    assert_eq!(shell.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(shell.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(shell.headers()["x-robots-tag"], "noindex");
    let html = String::from_utf8(
        to_bytes(shell.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
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

    // Hidden/draft/held content remains visible only to the privileged
    // reader while an immutable B/G sealed edition still exists.
    let _restore = ChapterStateCleanup { url, book, chapter };
    pg(
        url,
        &format!(
            "UPDATE books SET visibility='draft',admin_hold=TRUE WHERE id={book}; \
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
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_admin_chapter_access_audit \
                 WHERE admin_user_id={admin_id} AND chapter_id={chapter}"
            )
        ),
        3,
        "two sealed chapter grants and one PNG grant must be audited"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_admin_chapter_access_audit \
                 WHERE admin_user_id={admin_id} AND version_id={version} \
                 AND action='sealed_chapter_grant'"
            )
        ),
        2
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_admin_chapter_access_audit \
                 WHERE admin_user_id={admin_id} AND version_id={version} \
                 AND action='sealed_asset_grant'"
            )
        ),
        1
    );
    // Historical sealed inspection is an independent, explicit A/D/B/G/D
    // flow. The exact version and each PNG must stay bound to this chapter.
    let sealed_list = format!("/api/v1/reader/chapters/{chapter}/admin-sealed-history");
    let sealed_grant = format!("{sealed_list}/{version}/delivery-grants");
    let sealed_asset = format!("{sealed_list}/{version}/assets/{ASSET_UUID}/delivery-grants");
    let listed = send(
        grants,
        &sealed_list,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(listed["schema"], "tor.admin_sealed_history.v1");
    assert!(listed["versions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| { v["version_id"].as_str() == Some(version.to_string().as_str()) }));
    for target in [&sealed_list, &sealed_grant, &sealed_asset] {
        for (session, token, origin, denied) in [
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
            let response = send(grants, target, session, token, origin, "{}").await;
            assert_eq!(response.status(), denied, "{target}");
            assert!(to_bytes(response.into_body(), 8192)
                .await
                .unwrap()
                .is_empty());
        }
    }
    let invalid_version = send(
        grants,
        &format!("{sealed_list}/999999999/delivery-grants"),
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_ne!(invalid_version.status(), StatusCode::OK);
    assert!(to_bytes(invalid_version.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());
    let historic = send(
        grants,
        &sealed_grant,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(historic.status(), StatusCode::OK);
    assert_eq!(
        historic.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let historic: Value =
        serde_json::from_slice(&to_bytes(historic.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(historic["version_id"], version.to_string());
    assert!(!historic.to_string().contains(BODY));
    let image = send(
        grants,
        &sealed_asset,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        "{}",
    )
    .await;
    assert_eq!(image.status(), StatusCode::OK);
    let image: Value =
        serde_json::from_slice(&to_bytes(image.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(image["version_id"], version.to_string());
    assert_eq!(image["object_id"], ASSET_UUID);
    assert_eq!(fake.signed.load(Ordering::SeqCst), 5);
    assert_eq!(
        scalar(
            url,
            &format!(
            "SELECT count(*) FROM reader_admin_chapter_access_audit WHERE admin_user_id={admin_id}"
        )
        ),
        5
    );

    // Even held/draft sealed content remains reachable from the same
    // canonical URL, without disclosing its identity to a guest.
    let held_redirect = read_page(reader, &canonical, Some(&admin_session)).await;
    assert_eq!(held_redirect.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        held_redirect.headers()[header::LOCATION],
        format!("/read/admin/{chapter}")
    );
    assert_eq!(
        read_page(reader, &canonical, Some(&outsider))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // NEwBOO's administrator bypass is bound to the CURRENT version.
    // A sealed object from the history is not a reader fallback when neither
    // a working draft nor a published pointer currently selects it.
    pg(
        url,
        &format!("DELETE FROM chapter_publications WHERE chapter_id={chapter}"),
    );
    let before_orphan = fake.signed.load(Ordering::SeqCst);
    let orphan = send(
        grants,
        &path,
        Some(&admin_session),
        Some(&csrf),
        Some(ORIGIN),
        r#"{"source":"administrator"}"#,
    )
    .await;
    assert_ne!(orphan.status(), StatusCode::OK);
    assert_eq!(fake.signed.load(Ordering::SeqCst), before_orphan);
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );

    // Revocation must invalidate the next admin GET/POST, including a held
    // chapter; no new S3 bearer or a decrypted edition may be delivered.
    pg(
        url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE \
             WHERE user_id={admin_id} AND role='administrator'"
        ),
    );
    assert_eq!(
        read_page(reader, &page, Some(&admin_session))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        authority
            .sealed_historical_version(admin_actor, chapter_id, version_id)
            .await
            .unwrap(),
        None,
        "revoked administrator must lose access to historical sealed versions",
    );
    assert!(
        authority
            .sealed_history(admin_actor, chapter_id)
            .await
            .unwrap()
            .is_empty(),
        "revoked administrator cannot enumerate historical sealed versions"
    );
    // A revoked role cannot keep navigating through the canonical URL.
    assert_eq!(
        read_page(reader, &canonical, Some(&admin_session))
            .await
            .status(),
        StatusCode::NOT_FOUND
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
    assert_eq!(fake.signed.load(Ordering::SeqCst), 5);
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_admin_chapter_access_audit \
                 WHERE admin_user_id={admin_id}"
            )
        ),
        5,
        "rejected/revoked reads must not create successful-delivery audit entries"
    );
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
