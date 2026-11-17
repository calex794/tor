//! BR-060/D-05: preserve historical paid Layer Book rights across ordinary
//! deletion while denying a new buyer that same deleted Book. The old paid
//! term is not a direct pinned V1 or a new purchase of the deleted Book.
use super::*;
use tor_commerce::{
    domain::{
        layers::LayerCheckoutCommand,
        money::{IdempotencyKey, MoneyMinor, WalletId},
    },
    repositories::{layer_entitlements::PgLayerChapterEntitlements, layers::PgLayers},
};
use tor_contracts::paid_delivery::{Actor, EntitlementDecision, EntitlementPort};

pub(super) struct ActiveLayerProbe {
    accounts: PgAccounts,
    subscriber: i64,
    cookie: String,
    csrf: String,
    term_id: i64,
    layer_id: i64,
    offer_id: i64,
    layer_version: i64,
    published_version: i64,
    wallet_after_minor: i64,
    surviving_chapter: i64,
    surviving_version: i64,
}

pub(super) async fn before_deletion(
    app: &Router,
    url: &str,
    accounts: &PgAccounts,
    book: i64,
    chapter: i64,
) -> ActiveLayerProbe {
    let owner = scalar(
        url,
        &format!("SELECT owner_user_id FROM books WHERE id={book}"),
    );
    let current_version = scalar(
        url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
    );
    let (subscriber, cookie) = user(accounts, "br060-active-layer@tor-ci.example").await;
    let csrf = generate_session_token();
    pg(
        url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) \
             VALUES({subscriber},300,'RUB')"
        ),
    );

    let repo = PgLayers::new(accounts.pool().clone());
    let layer = repo
        .create_layer(PositiveId::new(owner).unwrap(), "BR060 Active Layer")
        .await
        .unwrap();
    let first_layer_version = repo
        .add_book(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(book).unwrap(),
        )
        .await
        .unwrap();
    // A package may include more than one Book. Deleting a single Book must
    // never revoke access to a still-live sibling Book or reverse the entire
    // purchase. The sibling needs a real published CURRENT version for D.
    let sibling_book = scalar(
        url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level) \
             VALUES('ci:br060-layer-surviving-book','ru','BR060 surviving Book','public',{owner},'basic') \
             RETURNING id"
        ),
    );
    let sibling_chapter = scalar(
        url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({sibling_book},1,'Surviving Layer chapter','public','paid',50) RETURNING id"
        ),
    );
    let sibling_version = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({sibling_chapter},1,'Surviving CURRENT edition') RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) \
             VALUES({sibling_chapter},{sibling_version})"
        ),
    );
    let layer_version = repo
        .add_book(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            PositiveId::new(sibling_book).unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        layer_version, first_layer_version,
        "adding another Book must advance Layer composition version"
    );
    let offer = repo
        .publish_offer(
            PositiveId::new(owner).unwrap(),
            PositiveId::new(layer).unwrap(),
            90,
            false,
        )
        .await
        .unwrap();
    let receipt = repo
        .buy(&LayerCheckoutCommand {
            actor: PositiveId::new(subscriber).unwrap(),
            wallet: WalletId::new(subscriber).unwrap(),
            layer: PositiveId::new(layer).unwrap(),
            offer: PositiveId::new(offer).unwrap(),
            version: PositiveId::new(layer_version).unwrap(),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "C".repeat(42))).unwrap(),
        })
        .await
        .unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.balance_after_minor, 210);
    assert_surviving_book_entitlement(accounts, subscriber, sibling_chapter, sibling_version).await;

    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM library_purchased_chapters \
                 WHERE user_id={subscriber} AND chapter_id={chapter}"
            ),
        ),
        0,
        "Layer access must never mint an individual direct V1 purchase"
    );
    let before = post_grant(
        app,
        chapter,
        Some(&cookie),
        Some(&csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(before.status(), StatusCode::OK);
    let issued = decode(before).await;
    assert_eq!(issued["version_id"], current_version.to_string());
    assert!(issued["content"]["ciphertext_url"]
        .as_str()
        .unwrap()
        .starts_with(ENDPOINT));
    assert!(!issued.to_string().contains(MARKDOWN));
    assert_eq!(
        private_get(app, &format!("/read/layer/{chapter}"), Some(&cookie))
            .await
            .status(),
        StatusCode::OK
    );
    ActiveLayerProbe {
        accounts: accounts.clone(),
        subscriber,
        cookie,
        csrf,
        term_id: receipt.term_id,
        layer_id: layer,
        offer_id: offer,
        layer_version,
        published_version: current_version,
        wallet_after_minor: receipt.balance_after_minor,
        surviving_chapter: sibling_chapter,
        surviving_version: sibling_version,
    }
}

pub(super) async fn after_deletion(
    app: &Router,
    url: &str,
    book: i64,
    chapter: i64,
    active: ActiveLayerProbe,
) {
    assert_eq!(
        pg(
            url,
            &format!("SELECT lifecycle_status FROM books WHERE id={book}")
        ),
        "ordinary_deleted"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM commerce_layer_terms t \
                 WHERE t.id={} AND t.user_id={} AND t.started_at<=now() \
                   AND (t.expires_at IS NULL OR t.expires_at>now()) \
                   AND NOT EXISTS (SELECT 1 FROM commerce_layer_term_revocations r \
                                   WHERE r.term_id=t.id)",
                active.term_id, active.subscriber
            ),
        ),
        1,
        "ordinary deletion must not forge a term refund or revocation"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT balance_minor FROM reader_wallet_accounts \
                 WHERE user_id={}",
                active.subscriber
            ),
        ),
        active.wallet_after_minor,
        "deletion must not change an existing Layer payment"
    );
    assert_eq!(
        scalar(
            url,
            &format!(
                "SELECT COUNT(*) FROM library_purchased_chapters \
                 WHERE user_id={} AND chapter_id={chapter}",
                active.subscriber
            ),
        ),
        0,
        "a still-active Layer term cannot silently become a permanent V1 pin"
    );

    // This is the archive contract: the same paid term obtained the Book
    // before ordinary deletion. D may still deliver CURRENT's last published
    // ciphertext, without inventing an individual direct chapter purchase.
    let retained = post_grant(
        app,
        chapter,
        Some(&active.cookie),
        Some(&active.csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(retained.status(), StatusCode::OK);
    let grant = decode(retained).await;
    assert_eq!(grant["version_id"], active.published_version.to_string());
    assert!(!grant.to_string().contains(MARKDOWN));
    assert!(grant["content"]["ciphertext_url"]
        .as_str()
        .unwrap()
        .starts_with(ENDPOINT));
    let shell = private_get(app, &format!("/read/layer/{chapter}"), Some(&active.cookie)).await;
    assert_eq!(shell.status(), StatusCode::OK);
    assert!(html_body(shell).await.contains("data-layer-archive=\"1\""));
    assert_eq!(
        private_get(
            app,
            &format!("/api/v1/reader/layer/{chapter}/toc/window?after=1.{chapter}&limit=36"),
            Some(&active.cookie),
        )
        .await
        .status(),
        StatusCode::OK,
        "old Layer reader keeps a private paged archive TOC"
    );
    let old_library =
        html_body(private_get(app, "/me/library/layers", Some(&active.cookie)).await).await;
    assert!(old_library.contains(&format!("data-layer-archived-book-id=\"{book}\"")));
    assert!(old_library.contains("Удалена с сайта"));

    // This subscriber arrives AFTER Book deletion. The current package may
    // still carry an immutable historical Book ID, but their term may not
    // inherit content that disappeared before they purchased.
    let (late_buyer, late_cookie) =
        user(&active.accounts, "br060-layer-late-buyer@tor-ci.example").await;
    pg(
        url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency) \
             VALUES({late_buyer},300,'RUB')"
        ),
    );
    let layer_repo = PgLayers::new(active.accounts.pool().clone());
    let new_term = layer_repo
        .buy(&LayerCheckoutCommand {
            actor: PositiveId::new(late_buyer).unwrap(),
            wallet: WalletId::new(late_buyer).unwrap(),
            layer: PositiveId::new(active.layer_id).unwrap(),
            offer: PositiveId::new(active.offer_id).unwrap(),
            version: PositiveId::new(active.layer_version).unwrap(),
            expected_price: MoneyMinor::new(90).unwrap(),
            key: IdempotencyKey::parse(&format!("{}8", "E".repeat(42))).unwrap(),
        })
        .await
        .unwrap();
    assert!(!new_term.replayed);
    assert_eq!(new_term.balance_after_minor, 210);
    let late_grant = post_grant(
        app,
        chapter,
        Some(&late_cookie),
        Some(&active.csrf),
        ORIGIN,
        r#"{"source":"layer"}"#,
    )
    .await;
    assert_eq!(late_grant.status(), StatusCode::NOT_FOUND);
    assert!(to_bytes(late_grant.into_body(), 8192)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        private_get(app, &format!("/read/layer/{chapter}"), Some(&late_cookie))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let late_library =
        html_body(private_get(app, "/me/library/layers", Some(&late_cookie)).await).await;
    assert!(!late_library.contains(&format!("data-layer-archived-book-id=\"{book}\"")));
    assert_surviving_book_entitlement(
        &active.accounts,
        late_buyer,
        active.surviving_chapter,
        active.surviving_version,
    )
    .await;

    // A site/legal block is stronger than both historical rights and age
    // eligibility. Lifting that block restores the same original term.
    pg(
        url,
        &format!("UPDATE books SET visibility_status='blocked' WHERE id={book}"),
    );
    assert_eq!(
        post_grant(
            app,
            chapter,
            Some(&active.cookie),
            Some(&active.csrf),
            ORIGIN,
            r#"{"source":"layer"}"#,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    pg(
        url,
        &format!("UPDATE books SET visibility_status='active' WHERE id={book}"),
    );
    assert_eq!(
        private_get(app, &format!("/read/layer/{chapter}"), Some(&active.cookie))
            .await
            .status(),
        StatusCode::OK
    );
    assert_surviving_book_entitlement(
        &active.accounts,
        active.subscriber,
        active.surviving_chapter,
        active.surviving_version,
    )
    .await;
}

async fn assert_surviving_book_entitlement(
    accounts: &PgAccounts,
    subscriber: i64,
    chapter: i64,
    version: i64,
) {
    // Query D's actual entitlement resolver: no S3 object or extra signed
    // grant is needed for this surviving-Book isolation regression.
    let result = PgLayerChapterEntitlements::new(accounts.pool().clone())
        .resolve_entitlement(
            Actor {
                user_id: PositiveId::new(subscriber).unwrap(),
            },
            PositiveId::new(chapter).unwrap(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        EntitlementDecision::Allowed {
            pinned: ChapterVersionRef {
                chapter_id: PositiveId::new(chapter).unwrap(),
                version_id: PositiveId::new(version).unwrap(),
            }
        },
        "ordinary deletion of another Book must not confiscate valid Layer CURRENT rights"
    );
}
