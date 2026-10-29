//! A real PostgreSQL Layer purchase through the already configured S3 grant.
use super::*;
use tor_commerce::{domain::layers::LayerCheckoutCommand, repositories::layers::PgLayers};

pub(super) async fn assert_layer_s3_grants(
    repo: PgLayers,
    app: &Router,
    url: &str,
    users: (i64, i64),
    edition: (i64, i64, i64),
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
    assert_eq!(
        scalar(url, &format!(
            "SELECT COUNT(*) FROM commerce_layer_term_first_exposures WHERE term_id={}",
            paid.term_id
        )),
        0,
        "checkout, metadata and a reader shell are not paid content grants"
    );
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
    assert_eq!(
        scalar(url, &format!(
            "SELECT COUNT(*) FROM commerce_layer_term_first_exposures WHERE term_id={}",
            paid.term_id
        )),
        0,
        "rendering the Layer reader shell must not mark first paid exposure"
    );

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
    assert_eq!(
        scalar(url, &format!(
            "SELECT COUNT(*) FROM commerce_layer_term_first_exposures WHERE term_id={}",
            paid.term_id
        )),
        0,
        "rejected grant requests must not count as content issuance"
    );
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
    assert_eq!(
        pg(url, &format!(
            "SELECT chapter_id || ':' || version_id || ':' || object_kind
             FROM commerce_layer_term_first_exposures WHERE term_id={}",
            paid.term_id
        )),
        format!("{chapter}:{published}:chapter"),
        "only the successfully assembled protected grant may record first exposure"
    );
    let first_at = pg(
        url,
        &format!(
            "SELECT granted_at::text FROM commerce_layer_term_first_exposures WHERE term_id={}",
            paid.term_id
        ),
    );
    // An omitted source can fall back to Layer after direct V1 is denied.
    // It must not bypass the exposure barrier or overwrite the first fact.
    let fallback = post_grant(
        app,
        chapter,
        Some(cookie),
        Some(csrf),
        ORIGIN,
        "{}",
    )
    .await;
    assert_eq!(fallback.status(), StatusCode::OK);
    assert_eq!(
        pg(url, &format!(
            "SELECT granted_at::text FROM commerce_layer_term_first_exposures WHERE term_id={}",
            paid.term_id
        )),
        first_at
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
    body
}
