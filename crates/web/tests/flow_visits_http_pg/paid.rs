//! Retired collection keeps actual paid/Layer entitlements and delivery boundaries.
//! The money/entitlement paths are real PG transactions. G always rejects:
//! this test cannot pretend that ciphertext was downloaded or decrypted.
use super::*;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tor_accounts::services::security::generate_session_token;
use tor_books::repositories::paid_storage::{PaidPublication, PgPaidStorage};
use tor_commerce::{
    domain::{
        layers::LayerCheckoutCommand,
        money::{CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId},
    },
    repositories::{checkout::PgCheckout, layers::PgLayers},
};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, DeliveryError, EncryptedObjectRef, GrantedContent, ObjectId, PositiveId,
    PrivateObjectDeliveryPort,
};

#[derive(Default)]
struct UnavailableDelivery(AtomicUsize);

#[async_trait]
impl PrivateObjectDeliveryPort for UnavailableDelivery {
    async fn grant_verified_object(
        &self,
        _: &EncryptedObjectRef,
        _: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(DeliveryError::Unavailable)
    }
}

fn positive(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn paid_and_layer_views_need_actual_entitlements_and_live_session() {
    for name in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
        "TOR_LAYER_READER_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let delivery = Arc::new(UnavailableDelivery::default());
    let app = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
        (delivery.clone(), "http://127.0.0.1:9000".to_owned()),
        None,
    )
    .unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let (_, owner) = registration(
        &app,
        &url,
        &format!("flow-paid-owner-{stamp}@tor-ci.example"),
    )
    .await;
    let (buyer_browser, buyer) = registration(
        &app,
        &url,
        &format!("flow-paid-buyer-{stamp}@tor-ci.example"),
    )
    .await;
    let (outsider_browser, outsider) = registration(
        &app,
        &url,
        &format!("flow-paid-outsider-{stamp}@tor-ci.example"),
    )
    .await;
    legacy_scope(&url, buyer);
    legacy_scope(&url, outsider);
    let buyer_before = optional_snapshot(&url, buyer);
    let outsider_before = optional_snapshot(&url, outsider);
    pg(&url, &format!("INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) VALUES({buyer},400,'RUB')"));
    let book: i64 = pg(&url, &format!("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
        VALUES('ci:flow:paid:{stamp}','ru','FLOW paid fixture','public',{owner},'basic') RETURNING id")).parse().unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
        VALUES({book},1,'Paid fixture','public','paid',175) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
        VALUES({chapter},1,'FLOW-PRIVATE-PAID-CONTENT') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    // This is registry metadata for a server-side shell test, not a real S3 object.
    let object = pg(&url, "SELECT gen_random_uuid()::text");
    let digest = hex(&Sha256::digest(b"FLOW-PRIVATE-PAID-CONTENT"));
    let aad = hex(
        format!("tor.encrypted.chapter.v1\n{chapter}\n{version}\n{object}\n{digest}\n").as_bytes(),
    );
    pg(&url, &format!("INSERT INTO storage_objects(id,object_kind,bucket,object_key,state,size_bytes,
        sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,kek_id,object_nonce,object_aad,codec,verified_at,delete_after)
        VALUES('{object}'::uuid,'chapter','tor-private-chapter-ci','private/chapter-versions/{object}.enc','ready',64,
        decode(repeat('04',32),'hex'),decode('{digest}','hex'),decode(repeat('08',48),'hex'),decode(repeat('03',12),'hex'),
        'content-kek-v1',decode(repeat('02',12),'hex'),decode('{aad}','hex'),'gzip',now(),now()+interval '24 hours')"));
    let pin = ChapterVersionRef {
        chapter_id: positive(chapter),
        version_id: positive(version),
    };
    PgPaidStorage::new(pool.clone())
        .attach_verified_and_publish(
            owner,
            book,
            pin,
            &ObjectId::parse(&object).unwrap(),
            PaidPublication::KeepCurrent,
        )
        .await
        .unwrap();
    let layer_path = format!("/read/layer/{chapter}");
    assert_eq!(
        buyer_browser.get(&app, &layer_path).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(own_pages(&url, buyer), 0);
    let receipt = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                positive(buyer),
                WalletId::new(buyer).unwrap(),
                pin,
                MoneyMinor::new(175).unwrap(),
                IdempotencyKey::parse(&generate_session_token()).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let paid_path = format!(
        "/read/paid/{chapter}?purchase_id={}",
        receipt.purchase_id.get()
    );
    for path in [&paid_path, &layer_path] {
        assert_eq!(
            Browser::default().get(&app, path).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            outsider_browser.get(&app, path).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = buyer_browser.get(&app, &paid_path).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("data-paid-chapter-reader"));
    assert!(!html.contains("FLOW-PRIVATE-PAID-CONTENT"));
    assert_eq!(own_pages(&url, buyer), 0);
    assert_eq!(
        buyer_browser.get(&app, "/me/purchases").await.status(),
        StatusCode::OK
    );
    assert_eq!(own_pages(&url, buyer), 0);
    let layers = PgLayers::new(pool.clone());
    let layer = layers
        .create_layer(positive(owner), "FLOW Layer fixture")
        .await
        .unwrap();
    let layer_version = layers
        .add_book(positive(owner), positive(layer), positive(book))
        .await
        .unwrap();
    let offer = layers
        .publish_offer(positive(owner), positive(layer), 90, false)
        .await
        .unwrap();
    layers
        .buy(&LayerCheckoutCommand {
            actor: positive(buyer),
            wallet: WalletId::new(buyer).unwrap(),
            layer: positive(layer),
            offer: positive(offer),
            version: positive(layer_version),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&generate_session_token()).unwrap(),
        })
        .await
        .unwrap();
    let response = buyer_browser.get(&app, &layer_path).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("data-layer-chapter-reader"));
    assert!(!html.contains("FLOW-PRIVATE-PAID-CONTENT"));
    assert_eq!(own_pages(&url, buyer), 0);
    assert_eq!(own_pages(&url, outsider), 0);
    pg(
        &url,
        &format!("UPDATE books SET access_level='adult' WHERE id={book}"),
    );
    for path in [&paid_path, &layer_path] {
        assert!(
            matches!(
                buyer_browser.get(&app, path).await.status(),
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ),
            "age-gated paid or Layer HTML must deny access"
        );
    }
    pg(
        &url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    assert_eq!(
        buyer_browser.get(&app, &paid_path).await.status(),
        StatusCode::OK
    );
    assert_eq!(own_pages(&url, buyer), 0);
    assert_eq!(
        delivery.0.load(Ordering::SeqCst),
        0,
        "HTML pageviews never grant or read S3 content"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer}")
        ),
        "135"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_wallet_entries WHERE user_id={buyer}")
        ),
        "2"
    );
    retired_choice(&buyer_browser, &app, FLOW_FORM, false).await;
    assert_eq!(own_pages(&url, buyer), 0);
    pg(
        &url,
        &format!("UPDATE sessions SET created_at=now()-interval '2 seconds', expires_at=now()-interval '1 second' WHERE user_id={buyer}"),
    );
    for path in [&paid_path, &layer_path] {
        assert_eq!(
            buyer_browser.get(&app, path).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    retired_choice(&outsider_browser, &app, FLOW_FORM, false).await;
    assert_eq!(own_pages(&url, outsider), 0);
    assert_eq!(optional_snapshot(&url, buyer), buyer_before);
    assert_eq!(optional_snapshot(&url, outsider), outsider_before);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_starts WHERE user_id IN({buyer},{outsider})"
            )
        ),
        "0"
    );
    // Financial evidence is immutable. Keep its fixture rows, hidden from feeds.
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
}
