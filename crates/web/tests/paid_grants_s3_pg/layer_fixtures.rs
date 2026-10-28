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
) {
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
}
