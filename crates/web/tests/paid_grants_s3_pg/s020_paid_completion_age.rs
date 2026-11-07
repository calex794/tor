//! S020/P0 #486: only a CURRENT B age grant can complete an already purchased
//! encrypted V1. D possession, G ciphertext and C completion remain independent.
//! Runs inside the real isolated S034 MinIO/PostgreSQL HTTP acceptance fixture.
use super::*;

async fn assert_rejected(
    app: &Router,
    edition: (i64, i64, i64),
    session: &str,
    csrf: &str,
    command: &Value,
) {
    let (chapter, purchase, _) = edition;
    assert_eq!(
        paid_page(app, chapter, purchase, session).await.status(),
        StatusCode::FORBIDDEN,
        "paid reader shell must check the current Book age tier",
    );
    let state = request_completion(app, "GET", edition, session, csrf, None).await;
    assert_eq!(
        state.status(),
        StatusCode::NOT_FOUND,
        "completion state must not reveal purchased metadata after age revocation",
    );
    assert_eq!(
        request_completion(app, "POST", edition, session, csrf, Some(command.clone()))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "a valid saved anchor cannot bypass revoked age access",
    );
}

async fn assert_readable(app: &Router, edition: (i64, i64, i64), session: &str, csrf: &str) {
    let (chapter, purchase, _) = edition;
    assert_eq!(
        paid_page(app, chapter, purchase, session).await.status(),
        StatusCode::OK,
    );
    let state = request_completion(app, "GET", edition, session, csrf, None).await;
    assert_eq!(state.status(), StatusCode::OK);
    let state = decode(state).await;
    assert_eq!(state["available"], true);
    assert_eq!(state["completed"], false);
}

// S020/P0: test a REAL B-attached encrypted V1 image, rather than an
// unknown UUID that would return 404 regardless of the live age permission.
// No previously signed S3 URL is reused; each POST must recheck D's authority.
async fn assert_private_asset_grant(
    app: &Router,
    edition: (i64, i64, i64),
    session: &str,
    csrf: &str,
    allowed: bool,
) {
    let (chapter, purchase, version) = edition;
    let request = Request::builder()
        .uri(format!(
            "/api/v1/reader/chapters/{chapter}/assets/{}/delivery-grants",
            asset_fixture::ASSET_OBJECT_ID
        ))
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header("x-csrf-token", csrf)
        .header(
            header::COOKIE,
            format!("tor-dev-session={session}; tor-dev-csrf={csrf}"),
        )
        .body(Body::from(
            json!({"purchase_id": purchase.to_string()}).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    if allowed {
        assert_eq!(response.status(), StatusCode::OK, "owned V1 image grant");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "private, no-store");
        let grant = decode(response).await;
        assert_eq!(grant["schema"], "tor.paid_asset_delivery.v1");
        assert_eq!(grant["version_id"], version.to_string());
        assert_eq!(grant["object_id"], asset_fixture::ASSET_OBJECT_ID);
        assert!(
            grant["content"]["ciphertext_url"]
                .as_str()
                .is_some_and(|url| url.starts_with(ENDPOINT)),
            "only a private MinIO ciphertext URL may be issued"
        );
        assert!(!grant.to_string().contains(MARKDOWN));
    } else {
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "a purchased V1 image must reject a missing/revoked age entitlement"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            to_bytes(response.into_body(), 8192).await.unwrap().is_empty(),
            "no ciphertext URL or wrapped key on age denial"
        );
    }
}

pub(super) async fn verify(
    app: &Router,
    url: &str,
    owner: (i64, i64),
    edition: (i64, i64, i64),
    session: &str,
    csrf: &str,
    command: &Value,
) {
    let (book, buyer) = owner;
    let (chapter, purchase, _) = edition;
    let facts_before = scalar(
        url,
        &format!(
            "SELECT count(*) FROM chapter_purchase_facts WHERE id={purchase} AND user_id={buyer}"
        ),
    );
    let owned_before = scalar(
        url,
        &format!(
            "SELECT count(*) FROM library_purchased_chapters
             WHERE user_id={buyer} AND chapter_id={chapter}"
        ),
    );
    assert_eq!(facts_before, 1, "fixture must own the immutable D purchase");
    assert_eq!(
        owned_before, 1,
        "fixture must own the historical purchased V1"
    );
    assert_private_asset_grant(app, edition, session, csrf, true).await;

    // Possession alone never grants a forbidden 18+ read or completion.
    pg(
        url,
        &format!("UPDATE books SET access_level='adult' WHERE id={book}"),
    );
    assert_rejected(app, edition, session, csrf, command).await;
    assert_private_asset_grant(app, edition, session, csrf, false).await;

    pg(
        url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={buyer}"),
    );
    assert_readable(app, edition, session, csrf).await;

    // 21+ is an independent server grant, not the 18+ opt-in.
    pg(
        url,
        &format!("UPDATE books SET access_level='pro_adult' WHERE id={book}"),
    );
    assert_rejected(app, edition, session, csrf, command).await;
    assert_private_asset_grant(app, edition, session, csrf, false).await;
    pg(
        url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({buyer},'pro_adult','moderation',TRUE)
             ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE"
        ),
    );
    pg(
        url,
        &format!("UPDATE users SET adult_enabled=FALSE WHERE id={buyer}"),
    );
    assert_readable(app, edition, session, csrf).await;
    assert_private_asset_grant(app, edition, session, csrf, true).await;

    pg(
        url,
        &format!(
            "UPDATE user_level_grants SET is_active=FALSE
             WHERE user_id={buyer} AND level='pro_adult'"
        ),
    );
    assert_rejected(app, edition, session, csrf, command).await;
    assert_private_asset_grant(app, edition, session, csrf, false).await;

    // Unknown legacy tier must not silently become basic.
    pg(
        url,
        &format!("UPDATE books SET access_level=NULL WHERE id={book}"),
    );
    assert_rejected(app, edition, session, csrf, command).await;
    assert_private_asset_grant(app, edition, session, csrf, false).await;

    // Age denial must not revoke the paid V1, manufacture a C completion,
    // or emit a fictitious F ranking/achievement outbox event.
    assert_eq!(
        scalar(
            url,
            &format!("SELECT count(*) FROM chapter_purchase_facts WHERE id={purchase} AND user_id={buyer}")
        ),
        facts_before,
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM library_purchased_chapters
                 WHERE user_id={buyer} AND chapter_id={chapter}"
            )
        ),
        owned_before,
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions
                 WHERE user_id={buyer} AND chapter_id={chapter}"
            )
        ),
        0,
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE dedupe_key='chapter-completed:{buyer}:{chapter}'"
            )
        ),
        0,
    );

    // Restore the same bought edition. The parent test must still be able
    // to produce exactly one genuine completion with its existing anchor.
    pg(
        url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    assert_readable(app, edition, session, csrf).await;
}
