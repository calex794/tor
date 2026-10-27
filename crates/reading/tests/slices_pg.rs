//! C-09/S006: disposable PostgreSQL concurrency and immutable paid versions.
use tor_reading::{
    repositories::{PgReadingProgress, PgReadingSync},
    services::{
        progress::{reader_blocks, valid_text_place, ReadingPlace, SavedPlaceState},
        sync::SyncError,
    },
};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn c_09_free_two_devices_cannot_rollback_or_reuse_revision() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test database");
    assert!(url.ends_with("/tor_ci"), "production must never be tested");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, email_normalized, display_name, password_hash) \
         OVERRIDING SYSTEM VALUE VALUES \
         (93501,'c09-free-a@example.invalid','Free A','$argon2id$fixture'), \
         (93502,'c09-free-b@example.invalid','Free B','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let sync = PgReadingSync::new(pool.clone());
    assert_eq!(sync.free_state(93501, 101).await.unwrap(), None);
    assert_eq!(sync.free_state(93502, 101).await.unwrap(), None);
    let first = ReadingPlace {
        block_index: 1,
        char_offset: 1,
    };
    let one = sync.save_free(93501, 101, 201, first, 0).await.unwrap();
    assert_eq!(one.revision, 1);
    assert!(one.updated_at.ends_with('Z'));
    assert_eq!(
        sync.save_free(93501, 101, 201, first, 0).await,
        Err(SyncError::Conflict)
    );
    assert_eq!(
        sync.free_state(93501, 101).await.unwrap().unwrap().place,
        first
    );

    // Two independent requests observe revision 1, then race on PostgreSQL.
    let a = sync.clone();
    let b = sync.clone();
    let (left, right) = tokio::join!(
        a.save_free(
            93501,
            101,
            201,
            ReadingPlace {
                block_index: 1,
                char_offset: 2
            },
            1,
        ),
        b.save_free(
            93501,
            101,
            201,
            ReadingPlace {
                block_index: 1,
                char_offset: 3
            },
            1,
        )
    );
    let wins = usize::from(left.is_ok()) + usize::from(right.is_ok());
    assert_eq!(wins, 1, "only one device may advance from revision 1");
    assert!(left == Err(SyncError::Conflict) || right == Err(SyncError::Conflict));
    let reconnected = PgReadingSync::new(pool.clone());
    let fresh = reconnected.free_state(93501, 101).await.unwrap().unwrap();
    assert_eq!(fresh.receipt.revision, 2);
    assert!((2..=3).contains(&fresh.place.char_offset));
    assert_eq!(
        sync.save_free(93501, 101, 201, first, 1).await,
        Err(SyncError::Conflict),
        "a late save must never roll back a newer revision"
    );
    assert_eq!(
        sync.save_free(93501, 101, 201, first, 2).await,
        Err(SyncError::Conflict),
        "even the current revision cannot move the cursor backward"
    );
    assert_eq!(
        sync.save_free(93502, 101, 201, first, 3).await,
        Err(SyncError::Conflict),
        "other user's missing cursor cannot be bootstrapped at revision > 0"
    );
    assert_eq!(
        sync.save_free(93501, 103, 203, first, 2).await,
        Err(SyncError::Conflict),
        "paid chapter cannot be saved by free route"
    );
    assert_eq!(sync.free_state(93502, 101).await.unwrap(), None);

    // Legacy HTML save uses the same revision counter and monotonic rule.
    let legacy = PgReadingProgress::new(pool.clone());
    legacy
        .save_position(
            93501,
            101,
            201,
            ReadingPlace {
                block_index: 1,
                char_offset: 4,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        sync.free_state(93501, 101)
            .await
            .unwrap()
            .unwrap()
            .receipt
            .revision,
        3
    );
    assert_eq!(
        sync.save_free(93501, 101, 201, first, 2).await,
        Err(SyncError::Conflict)
    );

    sqlx::query("DELETE FROM users WHERE id IN (93501,93502)")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn c_09_c_06_purchased_v1_survives_v2_but_not_revoke_or_hold() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test database");
    assert!(url.ends_with("/tor_ci"), "production must never be tested");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    // All fixture financial facts exist only in the disposable test database.
    // The public application still has no route to create a purchase.
    sqlx::query(
        "INSERT INTO users (id,email_normalized,display_name,password_hash) \
         OVERRIDING SYSTEM VALUE VALUES \
         (93601,'c09-paid-a@example.invalid','Paid A','$argon2id$fixture'), \
         (93602,'c09-paid-b@example.invalid','Paid B','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility) \
         VALUES(93610,'ci:c09:paid','ru','Закреплённая версия','public')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount) \
         VALUES(93620,93610,1,'Платная глава','public','paid',100)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown) \
         VALUES(93631,93620,1,'А😀Б\n\nПродолжение'), \
               (93632,93620,2,'Переписанная новая версия')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES (93620,93631)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_facts \
          (id,user_id,chapter_id,version_id,settlement_reference,amount_minor,currency) \
         VALUES (93640,93601,93620,93631,'ci-c09-legal-purchase',100,'RUB')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let sync = PgReadingSync::new(pool.clone());
    assert!(sync
        .purchased_allowed(93601, 93640, 93620, 93631)
        .await
        .unwrap());
    assert!(!sync
        .purchased_allowed(93602, 93640, 93620, 93631)
        .await
        .unwrap());
    assert_eq!(
        sync.purchased_state(93602, 93640, 93620, 93631).await,
        Err(SyncError::NotFound)
    );
    assert_eq!(
        sync.purchased_state(93601, 93640, 93620, 93632).await,
        Err(SyncError::NotFound)
    );
    let body = sync
        .purchased_body_for_validation(93601, 93640, 93620, 93631)
        .await
        .unwrap()
        .unwrap();
    let at_emoji = ReadingPlace {
        block_index: 1,
        char_offset: 3,
    };
    assert!(valid_text_place(at_emoji, &reader_blocks(&body)).is_ok());
    assert!(valid_text_place(
        ReadingPlace {
            block_index: 1,
            char_offset: 5
        },
        &reader_blocks(&body)
    )
    .is_err());
    let first = sync
        .save_purchased(93601, 93640, 93620, 93631, at_emoji, 0)
        .await
        .unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(
        sync.save_purchased(93602, 93640, 93620, 93631, at_emoji, 0)
            .await,
        Err(SyncError::NotFound)
    );
    assert_eq!(
        sync.save_purchased(93601, 93640, 93620, 93631, at_emoji, 0)
            .await,
        Err(SyncError::Conflict)
    );

    // A new publication MUST NOT silently move the purchase away from V1.
    sqlx::query("UPDATE chapter_publications SET version_id=93632 WHERE chapter_id=93620")
        .execute(&pool)
        .await
        .unwrap();
    let after_v2 = PgReadingSync::new(pool.clone());
    let saved = after_v2
        .purchased_state(93601, 93640, 93620, 93631)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.version_id, 93631);
    assert_eq!(saved.place, at_emoji);
    let next = after_v2
        .save_purchased(
            93601,
            93640,
            93620,
            93631,
            ReadingPlace {
                block_index: 2,
                char_offset: 1,
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(next.revision, 2);
    assert_eq!(
        sync.save_purchased(93601, 93640, 93620, 93631, at_emoji, 1)
            .await,
        Err(SyncError::Conflict),
        "late device cannot roll back purchased edition"
    );

    // Paid V1 survives withdrawal from the storefront. This must match D-03:
    // a creator's choice to stop NEW sales cannot silently erase old rights.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=93610")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=93620")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sync
        .purchased_allowed(93601, 93640, 93620, 93631)
        .await
        .unwrap());
    assert_eq!(
        after_v2
            .purchased_state(93601, 93640, 93620, 93631)
            .await
            .unwrap()
            .unwrap()
            .version_id,
        93631
    );
    sqlx::query("UPDATE books SET visibility='public' WHERE id=93610")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=93620")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=93620")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sync.purchased_state(93601, 93640, 93620, 93631).await,
        Err(SyncError::NotFound)
    );
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=93620")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) \
         VALUES (93640,'ci-c09-revocation','refund')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        sync.purchased_state(93601, 93640, 93620, 93631).await,
        Err(SyncError::NotFound)
    );
    assert_eq!(
        sync.save_purchased(
            93601,
            93640,
            93620,
            93631,
            ReadingPlace {
                block_index: 2,
                char_offset: 2
            },
            2
        )
        .await,
        Err(SyncError::NotFound)
    );
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn c_10_large_safe_reader_and_protected_versions() {
    use tor_reading::{
        domain::renderer::{render, valid_reader_length},
        repositories::PgReader,
        services::PublicReadingRepository,
    };
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test database");
    assert!(url.ends_with("/tor_ci"), "production must never be tested");
    let pool = sqlx::PgPool::connect(&url).await.expect("test database");
    let reader = PgReader::new(pool.clone());
    let live = reader.public_chapter(101).await.unwrap().unwrap();
    assert!(render(&live.body_markdown).contains("<strong>"));
    for denied in [102, 103, 104, 105, 106, 107] {
        assert!(
            reader.public_chapter(denied).await.unwrap().is_none(),
            "draft, paid, held and delayed chapters cannot return text: {denied}"
        );
    }

    let source = format!(
        "# Заголовок\n\n{}\n\n<svg onload=alert(1)>",
        "😀".repeat(49_800)
    );
    assert!(valid_reader_length(&source));
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility) \
         VALUES (96500,'ci:c10:long-reader','ru','Длинная книга','public')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount) \
         VALUES (96501,96500,1,'Большая глава','public','free',0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown) \
         VALUES (96502,96501,1,$1)",
    )
    .bind(&source)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES (96501,96502)")
        .execute(&pool)
        .await
        .unwrap();
    let chapter = reader.public_chapter(96501).await.unwrap().unwrap();
    assert_eq!(chapter.body_markdown, source);
    let html = render(&chapter.body_markdown);
    assert!(html.contains("😀😀"));
    assert!(html.contains("&lt;svg"));
    assert!(!html.contains("<svg"));
    assert!(!html.contains("onload=\""));
    assert!(
        html.len() < 1_000_000,
        "renderer amplification must be bounded"
    );
    let blocks = tor_reading::services::progress::reader_blocks(&chapter.body_markdown);
    assert!(blocks.len() <= 200);
    assert!(tor_reading::services::progress::valid_text_place(
        tor_reading::services::progress::ReadingPlace {
            block_index: 2,
            char_offset: 90_000,
        },
        &blocks,
    )
    .is_ok());
    sqlx::query("DELETE FROM books WHERE id=96500")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci"]
async fn c_09_free_v1_to_v2_drops_stale_anchor_not_history() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test database");
    assert!(url.ends_with("/tor_ci"), "production must never be tested");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users(id,email_normalized,display_name,password_hash) \
         OVERRIDING SYSTEM VALUE VALUES \
         (93701,'c09-version-reader@example.invalid','V1 Reader','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility) \
         VALUES (93710,'ci:c09:free-v2','ru','Free version switch','public')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount) \
         VALUES (93720,93710,1,'Free chapter','public','free',0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown) \
         VALUES (93731,93720,1,'First\n\nSecond'), \
                (93732,93720,2,'Rewritten first paragraph')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES (93720,93731)")
        .execute(&pool)
        .await
        .unwrap();

    let sync = PgReadingSync::new(pool.clone());
    let reading = PgReadingProgress::new(pool.clone());
    assert_eq!(
        reading.saved_state(93701, 93720, 93731).await.unwrap(),
        SavedPlaceState::NotSaved
    );
    let old = ReadingPlace {
        block_index: 2,
        char_offset: 3,
    };
    let saved = sync.save_free(93701, 93720, 93731, old, 0).await.unwrap();
    assert_eq!(saved.revision, 1);
    let before = reading.continue_reading(93701).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].block_index, 2);

    // An author releases V2. V1 is immutable, but its paragraph anchors no
    // longer identify positions in the current public text.
    sqlx::query("UPDATE chapter_publications SET version_id=93732 WHERE chapter_id=93720")
        .execute(&pool)
        .await
        .unwrap();
    let after = reading.continue_reading(93701).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].block_index, 0, "do not invent a V2 anchor from V1");
    assert_eq!(
        reading.saved_state(93701, 93720, 93732).await.unwrap(),
        SavedPlaceState::PreviousVersion
    );
    assert_eq!(
        reading.saved_place(93701, 93720, 93732).await.unwrap(),
        None
    );
    assert_eq!(
        sync.save_free(93701, 93720, 93731, old, 1).await,
        Err(SyncError::Conflict),
        "a delayed V1 device must not write over the new publication"
    );
    // The no-JavaScript HTML form resets to the first block in V2.
    // This must replace a V1 anchor even though the numeric position goes back.
    let reset = ReadingPlace {
        block_index: 1,
        char_offset: 0,
    };
    reading
        .save_position(93701, 93720, 93732, reset)
        .await
        .unwrap();
    let saved_reset = reading.saved_place(93701, 93720, 93732).await.unwrap();
    assert_eq!(saved_reset, Some(reset));
    assert_eq!(
        reading.saved_state(93701, 93720, 93732).await.unwrap(),
        SavedPlaceState::Current(reset)
    );
    let reset_state = sync.free_state(93701, 93720).await.unwrap().unwrap();
    assert_eq!(reset_state.version_id, 93732);
    assert_eq!(reset_state.place, reset);
    assert_eq!(reset_state.receipt.revision, 2);
    let fresh = ReadingPlace {
        block_index: 1,
        char_offset: 2,
    };
    let receipt = sync.save_free(93701, 93720, 93732, fresh, 2).await.unwrap();
    assert_eq!(receipt.revision, 3);
    let state = sync.free_state(93701, 93720).await.unwrap().unwrap();
    assert_eq!(state.version_id, 93732);
    assert_eq!(state.place, fresh);
    assert_eq!(
        reading.continue_reading(93701).await.unwrap()[0].block_index,
        1
    );
    assert_eq!(
        sync.save_free(93701, 93720, 93731, old, 3).await,
        Err(SyncError::Conflict),
        "old version cannot replace the new saved position"
    );

    sqlx::query("DELETE FROM users WHERE id=93701")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=93710")
        .execute(&pool)
        .await
        .unwrap();
}
