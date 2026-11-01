//! A real PostgreSQL Layer purchase through the already configured S3 grant.
use super::*;
use tor_commerce::{
    domain::{
        layers::LayerCheckoutCommand,
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::layers::PgLayers,
};

async fn post_asset_grant(
    app: &Router,
    chapter: i64,
    object_id: &str,
    cookie: &str,
    csrf: &str,
    body: &str,
) -> Response {
    let path = format!(
        "/api/v1/reader/chapters/{chapter}/assets/{object_id}/delivery-grants"
    );
    let request = Request::builder()
        .uri(path)
        .method("POST")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, format!("tor-dev-session={cookie}; tor-dev-csrf={csrf}"))
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

pub(super) async fn assert_layer_s3_grants(
    repo: PgLayers,
    app: &Router,
    url: &str,
    users: (i64, i64),
    edition: (i64, i64, i64),
    image_object: &str,
    credentials: (&str, &str),
) -> String {
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
    let csp = shell.headers()["content-security-policy"].to_str().unwrap();
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
    // Without an explicit source, D may select the same Layer term after
    // direct purchase lookup returns NotPurchased. Never force that fallback
    // through C-06's direct first-exposure journal.
    assert_eq!(
        post_grant(app, chapter, Some(cookie), Some(csrf), ORIGIN, "{}")
            .await
            .status(),
        StatusCode::OK,
        "default paid grant must permit an authorized Layer fallback"
    );
    // A Layer-only subscriber has no direct chapter purchase. Successful
    // Layer grants must not forge a D-06 first-exposure ledger entry.
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures \
                 WHERE user_id={subscriber} AND chapter_id={chapter}"
            ),
        ),
        0,
        "Layer-only grant must not create a direct purchase exposure"
    );
    // Paid assets use the same D-05 Layer authority as chapter ciphertext.
    // Explicit and implicit Layer grants never mint a D-06 direct exposure.
    for source in [r#"{"source":"layer"}"#, "{}"] {
        let image = post_asset_grant(app, chapter, image_object, cookie, csrf, source).await;
        assert_eq!(image.status(), StatusCode::OK, "Layer paid asset: {source}");
        assert_eq!(
            image.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let image_grant = decode(image).await;
        assert_eq!(image_grant["version_id"], published.to_string());
        assert!(
            image_grant["content"]["ciphertext_url"]
                .as_str()
                .unwrap()
                .starts_with(ENDPOINT)
        );
    }
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT count(*) FROM chapter_purchase_first_exposures \
                 WHERE user_id={subscriber} AND chapter_id={chapter}"
            ),
        ),
        0,
        "Layer paid illustration must not forge direct first-grant evidence"
    );
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
        post_asset_grant(
            app,
            chapter,
            image_object,
            cookie,
            csrf,
            r#"{"source":"layer"}"#,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "revoked Layer must never sign a paid illustration"
    );
    assert_eq!(
        private_get(app, &layer_page, Some(cookie)).await.status(),
        StatusCode::FORBIDDEN,
        "revoked Layer must never receive a fresh browser reader shell"
    );
    body
}
