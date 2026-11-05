//! S020/C-08: bookmarks must not become an age-tier metadata side channel.
use sqlx::PgPool;
use tor_reading::{repositories::PgLibrary, services::library::LibraryError};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_library_age_gate_rechecks_live_opt_in_grant_and_book_tier() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 library age','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-library-age-{nonce}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let library = PgLibrary::new(pool.clone());

    let mut ids = Vec::new();
    for (suffix, tier) in [
        ("basic", Some("basic")),
        ("adult", Some("adult")),
        ("pro", Some("pro_adult")),
        ("unknown", None),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES($1,'ru',$2,'public',$3) RETURNING id",
        )
        .bind(format!("ci:s020:library:{nonce}:{suffix}"))
        .bind(format!("Age library {nonce} {suffix}"))
        .bind(tier)
        .fetch_one(&pool)
        .await
        .unwrap();
        ids.push(id);
    }

    assert_eq!(library.add_book(user, ids[0]).await, Ok(()));
    for id in ids.iter().skip(1) {
        assert_eq!(
            library.add_book(user, *id).await,
            Err(LibraryError::NotFound)
        );
    }
    let first = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(
        first.items.iter().map(|b| b.book_id).collect::<Vec<_>>(),
        vec![ids[0]]
    );

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(library.add_book(user, ids[1]).await, Ok(()));
    assert_eq!(
        library.add_book(user, ids[2]).await,
        Err(LibraryError::NotFound)
    );
    assert_eq!(
        library.add_book(user, ids[3]).await,
        Err(LibraryError::NotFound)
    );

    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(library.add_book(user, ids[2]).await, Ok(()));
    let eligible = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(eligible.items.len(), 3);
    assert!(!eligible.items.iter().any(|b| b.book_id == ids[3]));

    // This is a live read gate, not an irreversible mutation of bookmarks.
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let revoked = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(revoked.items.len(), 2);
    assert!(!revoked.items.iter().any(|b| b.book_id == ids[2]));
    assert_eq!(
        library.add_book(user, ids[2]).await,
        Err(LibraryError::NotFound)
    );

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let no_opt_in = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(no_opt_in.items.len(), 1);
    assert_eq!(no_opt_in.items[0].book_id, ids[0]);
    assert_eq!(
        library.add_book(user, ids[1]).await,
        Err(LibraryError::NotFound)
    );

    sqlx::query("UPDATE user_level_grants SET is_active=TRUE WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let restored_pro = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(restored_pro.items.len(), 2, "21+ is independently granted");
    assert!(restored_pro.items.iter().any(|b| b.book_id == ids[2]));

    // An authorized viewer still cannot see an unclassified legacy Book.
    assert_eq!(
        library.add_book(user, ids[3]).await,
        Err(LibraryError::NotFound)
    );
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(ids[2])
        .execute(&pool)
        .await
        .unwrap();
    assert!(!library
        .list_books(user, 10, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|b| b.book_id == ids[2]));

    // Deleting a saved link remains permitted after eligibility is revoked.
    assert_eq!(library.remove_book(user, ids[2]).await, Ok(()));
    sqlx::query("UPDATE books SET visibility_status='active' WHERE id=$1")
        .bind(ids[2])
        .execute(&pool)
        .await
        .unwrap();
    assert!(!library
        .list_books(user, 10, None)
        .await
        .unwrap()
        .items
        .iter()
        .any(|b| b.book_id == ids[2]));

    // Existing personal bookmarks must not disappear from the private shelf
    // during ordinary pending deletion. A new bookmark is still not allowed.
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    let pending = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(pending.items.len(), 1);
    assert_eq!(pending.items[0].book_id, ids[0]);
    assert!(pending.items[0].pending_deletion);
    assert_eq!(
        library.add_book(user, ids[0]).await,
        Err(LibraryError::NotFound)
    );

    // Existing bookmarks do not override a hard admin hold or deletion.
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .list_books(user, 10, None)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=FALSE, is_deleted=TRUE WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .list_books(user, 10, None)
        .await
        .unwrap()
        .items
        .is_empty());
    sqlx::query(
        "UPDATE books SET is_deleted=FALSE, lifecycle_status='in_progress' WHERE id=$1",
    )
    .bind(ids[0])
    .execute(&pool)
    .await
    .unwrap();
    let restored = library.list_books(user, 10, None).await.unwrap();
    assert_eq!(restored.items.len(), 1);
    assert!(!restored.items[0].pending_deletion);

    // The Book tier is rechecked too: basic -> adult immediately hides the
    // previously saved book from a reader who has not opted in.
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(library
        .list_books(user, 10, None)
        .await
        .unwrap()
        .items
        .is_empty());

    sqlx::query("DELETE FROM reader_library_items WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
        .bind(&ids)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
