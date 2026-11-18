//! D-05: four chapters in one ordinarily removed Book, with independent
//! subscription-origin rights for the old, renewed, revoked and new buyer.
//! This is a PG-only entitlement/TOC matrix; S3 and genuine deletion HTTP
//! are exercised by the separate required paid-private-e2e/MinIO suites.
use super::*;
use tor_commerce::{
    repositories::layer_entitlements::PgLayerChapterEntitlements,
    services::reader_views::{LayerTocAccess, MixedTocCursor, MixedTocSide},
};
use tor_contracts::paid_delivery::{Actor, EntitlementDecision, EntitlementPort};

async fn buyer(pool: &PgPool, suffix: &str) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D05 chapter matrix','$argon2id$test') RETURNING id",
    )
    .bind(format!("d05-four-{suffix}@example.invalid"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1500,'RUB')",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn four_published_chapters(pool: &PgPool, book: i64) -> Vec<(i64, i64)> {
    let mut editions = Vec::new();
    for number in 1..=4_i32 {
        let (access, price) = if number <= 2 {
            ("free", 0)
        } else {
            ("paid", 50)
        };
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,$2,$3,'public',$4,$5) RETURNING id",
        )
        .bind(book)
        .bind(number)
        .bind(format!("Layer archived matrix chapter {number}"))
        .bind(access)
        .bind(price)
        .fetch_one(pool)
        .await
        .unwrap();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,$2) RETURNING id",
        )
        .bind(chapter)
        .bind(format!("Exact historical chapter number {number}"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter)
            .bind(version)
            .execute(pool)
            .await
            .unwrap();
        editions.push((chapter, version));
    }
    editions
}

fn purchase(actor: i64, layer: i64, offer: i64, version: i64, key: char) -> LayerCheckoutCommand {
    LayerCheckoutCommand {
        actor: id(actor),
        wallet: WalletId::new(actor).unwrap(),
        layer: id(layer),
        offer: id(offer),
        version: id(version),
        expected_price: MoneyMinor::new(90).unwrap(),
        key: IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    }
}

async fn assert_old_four(rights: &PgLayerChapterEntitlements, old: i64, editions: &[(i64, i64)]) {
    for &(chapter, version) in &editions[..2] {
        assert_eq!(
            rights
                .archived_free_version(id(old), id(chapter))
                .await
                .unwrap(),
            Some(id(version)),
        );
    }
    for &(chapter, version) in &editions[2..] {
        assert_eq!(
            rights
                .resolve_entitlement(Actor { user_id: id(old) }, id(chapter), None,)
                .await
                .unwrap(),
            EntitlementDecision::Allowed {
                pinned: tor_contracts::paid_delivery::ChapterVersionRef {
                    chapter_id: id(chapter),
                    version_id: id(version),
                },
            },
        );
    }
    // A private paid-chapter anchor must enumerate the two frozen FREE rows
    // and the other paid row without fabricating direct purchase history.
    let (anchor, _) = editions[2];
    let before = rights
        .layer_toc_window(
            id(old),
            id(anchor),
            MixedTocCursor {
                chapter_no: 3,
                chapter_id: anchor,
            },
            MixedTocSide::Before,
            10,
        )
        .await
        .unwrap();
    assert_eq!(before.chapters.len(), 2);
    assert!(before
        .chapters
        .iter()
        .all(|row| row.access == LayerTocAccess::ArchivedFree));
    assert!(before.chapters.iter().all(|row| editions[..2]
        .iter()
        .any(|&(chapter, _)| chapter == row.chapter_id)));
    let after = rights
        .layer_toc_window(
            id(old),
            id(anchor),
            MixedTocCursor {
                chapter_no: 3,
                chapter_id: anchor,
            },
            MixedTocSide::After,
            10,
        )
        .await
        .unwrap();
    assert_eq!(after.chapters.len(), 1);
    assert_eq!(after.chapters[0].chapter_id, editions[3].0);
    assert_eq!(after.chapters[0].access, LayerTocAccess::Subscription);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_four_chapter_archived_layer_preserves_only_predeletion_origin() {
    let (pool, layer_repo, owner, old, deleted_book) = setup("four-chapter-term-matrix").await;
    let editions = four_published_chapters(&pool, deleted_book).await;
    let surviving_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','Surviving book after deletion','public',$2,'basic')
         RETURNING id",
    )
    .bind("ci:d05-four-chapter-survivor")
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let surviving_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Surviving Layer paid chapter','public','paid',50) RETURNING id",
    )
    .bind(surviving_book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let surviving_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Surviving paid text') RETURNING id",
    )
    .bind(surviving_chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(surviving_chapter)
        .bind(surviving_version)
        .execute(&pool)
        .await
        .unwrap();

    let layer = layer_repo
        .create_layer(id(owner), "Four chapter archive matrix")
        .await
        .unwrap();
    layer_repo
        .add_book(id(owner), id(layer), id(deleted_book))
        .await
        .unwrap();
    let original_version = layer_repo
        .add_book(id(owner), id(layer), id(surviving_book))
        .await
        .unwrap();
    let offer = layer_repo
        .publish_offer(id(owner), id(layer), 90, false)
        .await
        .unwrap();
    let first = layer_repo
        .buy(&purchase(old, layer, offer, original_version, 'J'))
        .await
        .unwrap();
    let revoked = buyer(&pool, "revoked").await;
    let revocation_target = layer_repo
        .buy(&purchase(revoked, layer, offer, original_version, 'K'))
        .await
        .unwrap();
    // Lifetime uses an independent, non-exclusive Layer offer. A hard expiry
    // on the 30-day actors must not erase its permanent historic origin.
    let lifetime_buyer = buyer(&pool, "lifetime").await;
    let lifetime_layer = layer_repo
        .create_layer(id(owner), "Four chapter lifetime archive")
        .await
        .unwrap();
    layer_repo
        .add_book(id(owner), id(lifetime_layer), id(deleted_book))
        .await
        .unwrap();
    let lifetime_version = layer_repo
        .add_book(id(owner), id(lifetime_layer), id(surviving_book))
        .await
        .unwrap();
    let lifetime_offer = layer_repo
        .publish_offer(id(owner), id(lifetime_layer), 90, true)
        .await
        .unwrap();
    let lifetime = layer_repo
        .buy(&purchase(
            lifetime_buyer,
            lifetime_layer,
            lifetime_offer,
            lifetime_version,
            'P',
        ))
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT expires_at IS NULL FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(lifetime.term_id)
    .fetch_one(&pool)
    .await
    .unwrap());

    // This PG fixture models the verified SQL state of the actual owner
    // request. A separate test uses the real password/session/moderator API.
    let mut tx = pool.begin().await.unwrap();
    let deletion_revision: i64 = sqlx::query_scalar(
        "UPDATE books SET lifecycle_status='pending_deletion',
           lifecycle_revision=lifecycle_revision+1,visibility='draft'
         WHERE id=$1 RETURNING lifecycle_revision",
    )
    .bind(deleted_book)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_ordinary_deletion_requests
          (book_id,request_revision,owner_user_id,previous_lifecycle_status,previous_visibility)
         VALUES($1,$2,$3,'in_progress','public')",
    )
    .bind(deleted_book)
    .bind(deletion_revision)
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    for &(chapter, version) in &editions[..2] {
        sqlx::query(
            "INSERT INTO book_layer_free_archive_versions
             (book_id,request_revision,chapter_id,version_id)
             VALUES($1,$2,$3,$4)",
        )
        .bind(deleted_book)
        .bind(deletion_revision)
        .bind(chapter)
        .bind(version)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=clock_timestamp(),
         lifecycle_revision=lifecycle_revision+1 WHERE id=$1",
    )
    .bind(deleted_book)
    .execute(&pool)
    .await
    .unwrap();

    let rights = PgLayerChapterEntitlements::new(pool.clone());
    assert_old_four(&rights, old, &editions).await;
    assert_old_four(&rights, lifetime_buyer, &editions).await;
    // New checkout must never sell stale composition or include deleted book
    // in economics, even though a surviving Book remains in the same Layer.
    let late = buyer(&pool, "late").await;
    assert!(layer_repo
        .buy(&purchase(late, layer, offer, original_version, 'L'))
        .await
        .is_err());
    let clean_version = layer_repo
        .remove_book(id(owner), id(layer), id(deleted_book))
        .await
        .unwrap();
    assert_ne!(clean_version, original_version);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commerce_layer_version_books
             WHERE layer_version_id=$1 AND book_id=$2",
        )
        .bind(clean_version)
        .bind(deleted_book)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    let late_term = layer_repo
        .buy(&purchase(late, layer, offer, clean_version, 'M'))
        .await
        .unwrap();
    let renewed = layer_repo
        .buy(&purchase(old, layer, offer, clean_version, 'N'))
        .await
        .unwrap();
    assert_ne!(first.term_id, renewed.term_id);
    let (contiguous, same_origin): (bool, bool) = sqlx::query_as(
        "SELECT next.started_at=previous.expires_at,
                next.access_origin_version_id=previous.access_origin_version_id
         FROM commerce_layer_terms previous JOIN commerce_layer_terms next ON next.id=$2
         WHERE previous.id=$1",
    )
    .bind(first.term_id)
    .bind(renewed.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        contiguous && same_origin,
        "renewal must preserve continuous old origin"
    );
    assert_eq!(
        rights
            .resolve_entitlement(Actor { user_id: id(late) }, id(surviving_chapter), None,)
            .await
            .unwrap(),
        EntitlementDecision::Allowed {
            pinned: tor_contracts::paid_delivery::ChapterVersionRef {
                chapter_id: id(surviving_chapter),
                version_id: id(surviving_version),
            },
        },
        "new buyer retains paid rights to the surviving Book"
    );
    assert_old_four(&rights, old, &editions).await;
    for &(chapter, _) in &editions[..2] {
        assert_eq!(
            rights
                .archived_free_version(id(late), id(chapter))
                .await
                .unwrap(),
            None
        );
    }
    for &(chapter, _) in &editions[2..] {
        assert!(matches!(
            rights
                .resolve_entitlement(Actor { user_id: id(late) }, id(chapter), None)
                .await
                .unwrap(),
            EntitlementDecision::Denied { .. }
        ));
    }
    // Neither a later purchaser nor an expired subscriber may enumerate
    // the hidden book's FREE titles by asking for a paid-chapter TOC.
    let archive_anchor = editions[2].0;
    assert!(
        rights
            .layer_toc_window(
                id(late),
                id(archive_anchor),
                MixedTocCursor {
                    chapter_no: 3,
                    chapter_id: archive_anchor,
                },
                MixedTocSide::Before,
                10,
            )
            .await
            .is_err(),
        "new buyer must not enumerate deleted chapter titles"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commerce_layer_term_share_snapshots
             WHERE term_id=$1 AND book_id=$2",
        )
        .bind(late_term.term_id)
        .bind(deleted_book)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM library_purchased_chapters
             WHERE user_id=$1 AND chapter_id = ANY($2)",
        )
        .bind(old)
        .bind(
            editions[2..]
                .iter()
                .map(|(chapter, _)| *chapter)
                .collect::<Vec<_>>()
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "Layer membership must not mint direct purchases"
    );

    // Terms are immutable: DO NOT UPDATE dates just to simulate expiry.
    // Verify the exact 30-day horizon read-only, then exercise a genuine
    // append-only legal revocation against the same four-chapter entitlement.
    let (exact_30_days, expired_at_plus_31_days): (bool, bool) = sqlx::query_as(
        "SELECT expires_at=started_at+interval '30 days',
                expires_at<now()+interval '31 days'
         FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(revocation_target.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(exact_30_days && expired_at_plus_31_days);
    assert_old_four(&rights, revoked, &editions).await;
    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block',$2)",
    )
    .bind(revocation_target.term_id)
    .bind(format!(
        "ci:d05-four-legal-revoke:{}",
        revocation_target.term_id
    ))
    .execute(&pool)
    .await
    .unwrap();
    for &(chapter, _) in &editions[..2] {
        assert_eq!(
            rights
                .archived_free_version(id(revoked), id(chapter))
                .await
                .unwrap(),
            None
        );
    }
    for &(chapter, _) in &editions[2..] {
        assert!(matches!(
            rights
                .resolve_entitlement(
                    Actor {
                        user_id: id(revoked)
                    },
                    id(chapter),
                    None
                )
                .await
                .unwrap(),
            EntitlementDecision::Denied { .. }
        ));
    }
    assert!(
        rights
            .layer_toc_window(
                id(revoked),
                id(archive_anchor),
                MixedTocCursor {
                    chapter_no: 3,
                    chapter_id: archive_anchor,
                },
                MixedTocSide::Before,
                10,
            )
            .await
            .is_err(),
        "revoked buyer must not enumerate deleted chapter titles"
    );
    assert_old_four(&rights, old, &editions).await;
    assert_old_four(&rights, lifetime_buyer, &editions).await;

    // Administrative hold and logical hard-delete are stronger than historic
    // paid origin, including an uninterrupted renewed term and lifetime term.
    // Toggle only isolated B security flags; never rewrite purchase facts.
    for restricted_sql in [
        "UPDATE books SET admin_hold=TRUE WHERE id=$1",
        "UPDATE books SET is_deleted=TRUE WHERE id=$1",
    ] {
        sqlx::query(restricted_sql)
            .bind(deleted_book)
            .execute(&pool)
            .await
            .unwrap();
        for actor in [old, lifetime_buyer] {
            for &(chapter, _) in &editions[..2] {
                assert_eq!(
                    rights
                        .archived_free_version(id(actor), id(chapter))
                        .await
                        .unwrap(),
                    None,
                    "book-wide B restriction must deny archived FREE"
                );
            }
            for &(chapter, _) in &editions[2..] {
                assert!(
                    matches!(
                        rights
                            .resolve_entitlement(Actor { user_id: id(actor) }, id(chapter), None)
                            .await
                            .unwrap(),
                        EntitlementDecision::Denied { .. }
                    ),
                    "book-wide B restriction must deny historical PAID"
                );
            }
            assert!(
                rights
                    .layer_toc_window(
                        id(actor),
                        id(archive_anchor),
                        MixedTocCursor {
                            chapter_no: 3,
                            chapter_id: archive_anchor,
                        },
                        MixedTocSide::Before,
                        10,
                    )
                    .await
                    .is_err(),
                "restricted historic Layer cannot enumerate deleted Book titles"
            );
        }
        sqlx::query("UPDATE books SET admin_hold=FALSE, is_deleted=FALSE WHERE id=$1")
            .bind(deleted_book)
            .execute(&pool)
            .await
            .unwrap();
        assert_old_four(&rights, old, &editions).await;
        assert_old_four(&rights, lifetime_buyer, &editions).await;
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_elapsed_30d_historical_term_denies_four_chapters_with_valid_wallet_origin() {
    let (pool, layer_repo, owner, active, book) = setup("elapsed-term-matrix").await;
    let editions = four_published_chapters(&pool, book).await;

    // A wholly new disposable historical fixture is inserted with dates from
    // 32 days ago. No UPDATE of an immutable term, wallet entry, or Layer
    // composition is used to emulate expiry.
    let layer: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layers(owner_user_id,title,created_at)
         VALUES($1,'D05 elapsed 30-day archive',now()-interval '33 days') RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let origin: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layer_versions(layer_id,version_no,created_at)
         VALUES($1,1,now()-interval '32 days') RETURNING id",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO commerce_layer_version_books(layer_version_id,book_id) VALUES($1,$2)")
        .bind(origin)
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    let offer: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layer_offers
         (layer_id,revision,term_kind,price_minor,created_at)
         VALUES($1,1,'prepaid_30d',90,now()-interval '32 days') RETURNING id",
    )
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE commerce_layers SET current_version_id=$2,current_offer_id=$3 WHERE id=$1")
        .bind(layer)
        .bind(origin)
        .bind(offer)
        .execute(&pool)
        .await
        .unwrap();

    // A genuine new purchase proves that the very same Layer and four
    // publications remain readable to an unexpired buyer.
    let active_term = layer_repo
        .buy(&purchase(active, layer, offer, origin, 'U'))
        .await
        .unwrap();
    let expired = buyer(&pool, "elapsed").await;
    let command_key = format!("{}8", "W".repeat(42));

    // Construct the *initial* immutable record for an already elapsed term
    // in one transaction, including real B-10 shares, a matching idempotent
    // checkout command, an actual wallet debit and an exact journal reference.
    // Deferred PostgreSQL triggers prove these are not orphaned credentials.
    let mut tx = pool.begin().await.unwrap();
    let expired_term: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layer_terms
         (user_id,layer_id,offer_id,base_version_id,access_origin_version_id,
          term_kind,amount_minor,started_at,expires_at,purchased_at)
         VALUES($1,$2,$3,$4,$4,'prepaid_30d',90,
                now()-interval '31 days',now()-interval '1 day',
                now()-interval '31 days') RETURNING id",
    )
    .bind(expired)
    .bind(layer)
    .bind(offer)
    .bind(origin)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let share_revision: i64 =
        sqlx::query_scalar("SELECT current_revision FROM revenue_share_states WHERE book_id=$1")
            .bind(book)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO commerce_layer_term_share_snapshots
         (term_id,book_id,share_revision) VALUES($1,$2,$3)",
    )
    .bind(expired_term)
    .bind(book)
    .bind(share_revision)
    .execute(&mut *tx)
    .await
    .unwrap();
    let share_lines = sqlx::query(
        "INSERT INTO commerce_layer_term_share_lines
         (term_id,book_id,user_id,share_percent)
         SELECT $1,$2,user_id,share_percent FROM revenue_share_assignments
         WHERE book_id=$2 AND revision=$3",
    )
    .bind(expired_term)
    .bind(book)
    .bind(share_revision)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(share_lines.rows_affected() > 0);
    sqlx::query(
        "INSERT INTO commerce_layer_commands
         (user_id,command_key,layer_id,expected_offer_id,
          expected_version_id,expected_price_minor,term_id)
         VALUES($1,$2,$3,$4,$5,90,$6)",
    )
    .bind(expired)
    .bind(&command_key)
    .bind(layer)
    .bind(offer)
    .bind(origin)
    .bind(expired_term)
    .execute(&mut *tx)
    .await
    .unwrap();
    let balance: i64 = sqlx::query_scalar(
        "UPDATE reader_wallet_accounts SET balance_minor=balance_minor-90
         WHERE user_id=$1 AND currency='RUB' AND balance_minor>=90
         RETURNING balance_minor",
    )
    .bind(expired)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
         (user_id,kind,delta_minor,balance_after_minor,reference,layer_term_id)
         VALUES($1,'layer_purchase',-90,$2,$3,$4)",
    )
    .bind(expired)
    .bind(balance)
    .bind(format!("layer-wallet:{expired}:{command_key}"))
    .bind(expired_term)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let (valid_wallet, exactly_30_days, already_elapsed, physical_version_predates_expiry): (
        bool,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT d05_layer_term_has_wallet_evidence(t.id),
                t.expires_at=t.started_at+interval '30 days',
                t.expires_at<now(),
                v.created_at<t.expires_at
         FROM commerce_layer_terms t
         JOIN commerce_layer_versions v ON v.id=t.access_origin_version_id
         WHERE t.id=$1",
    )
    .bind(expired_term)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        valid_wallet && exactly_30_days && already_elapsed && physical_version_predates_expiry,
        "expiry must be the only denial reason, not missing financial provenance"
    );
    assert_eq!(balance, 1410);

    // The deleted Book is eligible for the active original buyer, and must
    // not appear even as a title in the expired buyer's private Layer TOC.
    let revision: i64 = sqlx::query_scalar(
        "UPDATE books SET lifecycle_status='pending_deletion',
         lifecycle_revision=lifecycle_revision+1,visibility='draft'
         WHERE id=$1 RETURNING lifecycle_revision",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_ordinary_deletion_requests
         (book_id,request_revision,owner_user_id,
          previous_lifecycle_status,previous_visibility)
         VALUES($1,$2,$3,'in_progress','public')",
    )
    .bind(book)
    .bind(revision)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    for &(chapter, version) in &editions[..2] {
        sqlx::query(
            "INSERT INTO book_layer_free_archive_versions
             (book_id,request_revision,chapter_id,version_id)
             VALUES($1,$2,$3,$4)",
        )
        .bind(book)
        .bind(revision)
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=clock_timestamp(),
         lifecycle_revision=lifecycle_revision+1 WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let rights = PgLayerChapterEntitlements::new(pool.clone());
    assert_old_four(&rights, active, &editions).await;
    assert!(!layer_repo
        .can_read_book(id(expired), id(book))
        .await
        .unwrap());
    for &(chapter, _) in &editions[..2] {
        assert_eq!(
            rights
                .archived_free_version(id(expired), id(chapter))
                .await
                .unwrap(),
            None,
            "expired 30-day term may not read frozen FREE chapter"
        );
    }
    for &(chapter, _) in &editions[2..] {
        assert!(matches!(
            rights
                .resolve_entitlement(
                    Actor {
                        user_id: id(expired)
                    },
                    id(chapter),
                    None
                )
                .await
                .unwrap(),
            EntitlementDecision::Denied { .. }
        ));
    }
    assert!(
        rights
            .layer_toc_window(
                id(expired),
                id(editions[2].0),
                MixedTocCursor {
                    chapter_no: 3,
                    chapter_id: editions[2].0,
                },
                MixedTocSide::Before,
                10,
            )
            .await
            .is_err(),
        "expired reader may not enumerate archived titles"
    );
    assert_old_four(&rights, active, &editions).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commerce_layer_term_revocations WHERE term_id=$1",
        )
        .bind(expired_term)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "expiry must deny without fabricating a revocation"
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT d05_layer_term_has_wallet_evidence($1)",)
            .bind(expired_term)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    assert_ne!(active_term.term_id, expired_term);
    pool.close().await;
}
