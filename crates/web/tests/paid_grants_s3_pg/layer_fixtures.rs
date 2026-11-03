//! A real PostgreSQL Layer purchase through the already configured S3 grant.
use super::*;
use tor_commerce::{
    domain::{
        layers::LayerCheckoutCommand,
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
};

async fn layer_asset_grant(
    app: &Router,
    chapter: i64,
    cookie: &str,
    csrf: &str,
    json: &str,
) -> Response {
    let request = Request::builder()
        .uri(format!(
            "/api/v1/reader/chapters/{chapter}/assets/{OBJECT_ID}/delivery-grants"
        ))
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header("x-csrf-token", csrf)
        .header(
            header::COOKIE,
            format!("tor-dev-session={cookie}; tor-dev-csrf={csrf}"),
        )
        .body(Body::from(json.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

pub(super) async fn assert_layer_s3_grants(
    repo: PgLayers,
    app: &Router,
    url: &str,
    users: (i64, i64),
    edition: (i64, i64, i64),
    credentials: (&str, &str),
) -> (String, Value, String) {
    let (author, subscriber) = users;
    let (book, chapter, published) = edition;
    let (cookie, csrf) = credentials;
    pg(
        url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) \
             VALUES({subscriber},300,'RUB')"
        ),
    );
    let layer_page = format!("/read/layer/{chapter}");
    assert_eq!(
        private_get(app, &layer_page, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::FORBIDDEN
    );
    let unowned = post_grant(
        app,
        chapter,
        Some(cookie),
        Some(csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(unowned.status(), StatusCode::FORBIDDEN);

    let layer = repo
        .create_layer(PositiveId::new(author).unwrap(), "S034 Layer")
        .await
        .unwrap();
    let version = repo
        .add_book(
            PositiveId::new(author).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(book).unwrap(),
        )
        .await
        .unwrap();
    let offer = repo
        .publish_offer(
            PositiveId::new(author).unwrap(),
            PositiveId::new(layer).unwrap(),
            90,
            false,
        )
        .await
        .unwrap();
    let paid = repo
        .buy(&LayerCheckoutCommand {
            actor: PositiveId::new(subscriber).unwrap(),
            wallet: WalletId::new(subscriber).unwrap(),
            layer: PositiveId::new(layer).unwrap(),
            offer: PositiveId::new(offer).unwrap(),
            version: PositiveId::new(version).unwrap(),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "A".repeat(42))).unwrap(),
        })
        .await
        .unwrap();
    assert!(!paid.replayed);
    assert_eq!(paid.balance_after_minor, 210);
    let shell = private_get(app, &layer_page, Some(cookie)).await;
    assert_eq!(shell.status(), StatusCode::OK);
    assert_eq!(shell.headers()[header::CACHE_CONTROL], "no-store");
    let csp = shell.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("connect-src 'self' http://127.0.0.1:9000"));
    assert!(csp.contains("frame-ancestors 'none'"));
    let body = html_body(shell).await;
    assert!(body.contains("data-layer-chapter-reader"));
    assert!(body.contains("data-chapter-id"));
    assert!(body.contains("newboo-layer-reader.mjs"));
    assert!(!body.contains("data-purchase-id"));
    assert!(!body.contains(MARKDOWN));
    assert!(!body.contains("dek_b64u"));
    assert!(!body.contains("ciphertext_url"));

    let mixed = post_grant(
        app,
        chapter,
        Some(cookie),
        Some(csrf),
        ORIGIN,
        r#"{"source":"layer","purchase_id":"999"}"#,
    )
    .await;
    assert_eq!(mixed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let granted = post_grant(
        app,
        chapter,
        Some(cookie),
        Some(csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(granted.status(), StatusCode::OK);
    assert_eq!(
        granted.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let payload = decode(granted).await;
    assert_eq!(payload["version_id"], published.to_string());
    assert_eq!(payload["schema"], "tor.paid_delivery.v1");
    assert!(
        payload["content"]["ciphertext_url"]
            .as_str()
            .unwrap()
            .starts_with(ENDPOINT),
        "only the configured private S3 origin may be signed"
    );
    assert!(!payload.to_string().contains(MARKDOWN));

    // This object's UUID belongs to encrypted chapter TEXT, not B's immutable
    // image membership. A valid Layer subscription cannot turn it into a
    // signed image bearer URL. Neither may a direct purchase ID be mixed in.
    let unknown_image =
        layer_asset_grant(app, chapter, cookie, csrf, r#"{"source":"layer"}"#).await;
    assert_eq!(unknown_image.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        to_bytes(unknown_image.into_body(), 8192)
            .await
            .unwrap()
            .len(),
        0,
        "missing B image membership must not expose S3 URLs"
    );
    assert_eq!(
        layer_asset_grant(
            app,
            chapter,
            cookie,
            csrf,
            r#"{"source":"layer","purchase_id":"999"}"#,
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
    );

    // S020/#486: a S3 grant must not reveal a signed URL or DEK when age
    // permission is absent. The D-05 PG suite covers the full age matrix.
    // HTTP probes deliberately remain within the production 12/min/actor cap.
    pg(
        url,
        &format!("UPDATE books SET access_level='adult' WHERE id={book}"),
    );
    let age_denied = post_grant(
        app,
        chapter,
        Some(cookie),
        Some(csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(age_denied.status(), StatusCode::NOT_FOUND);
    assert_eq!(age_denied.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        to_bytes(age_denied.into_body(), 8192)
            .await
            .unwrap()
            .is_empty(),
        "no DEK or signed URL without current 18+ permission"
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={subscriber}"),
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::OK
    );

    pg(
        url,
        &format!("UPDATE books SET access_level='pro_adult' WHERE id={book}"),
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({subscriber},'pro_adult','moderation',TRUE)"
        ),
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::OK
    );
    pg(
        url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={subscriber}"),
    );
    let revoked = post_grant(
        app,
        chapter,
        Some(cookie),
        Some(csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::NOT_FOUND);
    assert_eq!(revoked.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        to_bytes(revoked.into_body(), 8192)
            .await
            .unwrap()
            .is_empty(),
        "revoked 21+ grant must not expose any S3 bearer"
    );
    pg(
        url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );

    // A blocked storefront disallows new sales without confiscating
    // existing rights. The term, not the storefront, is the entitlement.
    pg(
        url,
        &format!("UPDATE commerce_layers SET status='blocked' WHERE id={layer}"),
    );
    assert_eq!(
        post_grant(
            app,
            chapter,
            Some(cookie),
            Some(csrf),
            ORIGIN,
            r#"{"source":"layer"}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    // C-12 / S065: a valid Layer term must not bypass Book or Chapter
    // administrative holds. Blocked Layer storefronts alone do not revoke
    // purchased rights; hard B holds independently stop new signed grants.
    for (scope, hold, clear) in [
        (
            "book",
            format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
            format!("UPDATE books SET admin_hold=FALSE WHERE id={book}"),
        ),
        (
            "chapter",
            format!("UPDATE chapters SET admin_hold=TRUE WHERE id={chapter}"),
            format!("UPDATE chapters SET admin_hold=FALSE WHERE id={chapter}"),
        ),
    ] {
        pg(url, &hold);
        let denied_grant = post_grant(
            app,
            chapter,
            Some(cookie),
            Some(csrf),
            ORIGIN,
            r#"{"source":"layer"}"#,
        )
        .await;
        assert_eq!(
            denied_grant.status(),
            StatusCode::NOT_FOUND,
            "{scope} hold must override a valid Layer term"
        );
        assert_eq!(denied_grant.headers()[header::CACHE_CONTROL], "no-store");
        assert!(
            to_bytes(denied_grant.into_body(), 8192)
                .await
                .unwrap()
                .is_empty(),
            "denied {scope} grant must contain no S3 credentials"
        );

        let denied_shell = private_get(app, &layer_page, Some(cookie)).await;
        assert_eq!(
            denied_shell.status(),
            StatusCode::NOT_FOUND,
            "{scope} hold must close the Layer reader shell"
        );
        assert!(
            to_bytes(denied_shell.into_body(), 8192)
                .await
                .unwrap()
                .is_empty(),
            "denied {scope} shell must contain no chapter text"
        );

        pg(url, &clear);
        if scope == "book" {
            // The signed-grant and reader-shell paths use the same Layer D policy.
            // Avoid exhausting the real per-actor quota in a single E2E sequence.
            assert_eq!(
                private_get(app, &layer_page, Some(cookie)).await.status(),
                StatusCode::OK,
                "lifting Book hold restores the existing Layer reader shell"
            );
        } else {
            assert_eq!(
                post_grant(
                    app,
                    chapter,
                    Some(cookie),
                    Some(csrf),
                    ORIGIN,
                    r#"{"source":"layer"}"#,
                )
                .await
                .status(),
                StatusCode::OK,
                "lifting Chapter hold restores the existing Layer signed grant"
            );
        }
    }

    pg(
        url,
        &format!(
            "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference) \
             VALUES({},'legal_block','ci:s034-layer-revoked')",
            paid.term_id
        ),
    );
    assert_eq!(
        post_grant(
            app,
            chapter,
            Some(cookie),
            Some(csrf),
            ORIGIN,
            r#"{"source":"layer"}"#,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "revoked Layer must never authorize another signed URL"
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::FORBIDDEN,
        "revoked Layer must never receive a fresh browser reader shell"
    );
    let revoked_image =
        layer_asset_grant(app, chapter, cookie, csrf, r#"{"source":"layer"}"#).await;
    assert_eq!(revoked_image.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        to_bytes(revoked_image.into_body(), 8192)
            .await
            .unwrap()
            .len(),
        0,
        "revoked Layer must not expose object credentials"
    );
    (body, payload, csp)
}
