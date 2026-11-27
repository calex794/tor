//! AUTH-READ: real PG persistence, owner CAS, and inactive rollout fence.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::{ManagedGuestReading, PgAuthoring},
    services::authoring::AuthorError,
};
use tor_contracts::guest_reading::GuestReadingPolicy;

async fn user_session(pool: &PgPool, label: &str, marker: u128) -> (i64, [u8; 32]) {
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Guest reading CI','$argon2id$test') RETURNING id",
    )
    .bind(format!("guest-reading-{label}-{marker}@example.test"))
    .fetch_one(pool)
    .await
    .unwrap();
    let mut digest = [0xA7; 32];
    digest[..16].copy_from_slice(&marker.to_be_bytes());
    digest[16..24].copy_from_slice(&user.to_be_bytes());
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,now()+INTERVAL '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    (user, digest)
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn auth_read_owner_policy_cas_and_closed_rollout() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let (owner, owner_session) = user_session(&pool, "owner", marker).await;
    let (translator, translator_session) = user_session(&pool, "translator", marker).await;
    let (_, outsider_session) = user_session(&pool, "outsider", marker).await;
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,access_level)
         VALUES($1,'ru','Guest reading fixture',$2,'basic') RETURNING id",
    )
    .bind(format!("auth-read:ci:{marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role) VALUES($1,$2,'translator')",
    )
    .bind(book)
    .bind(translator)
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgAuthoring::new(pool.clone());
    let initial = ManagedGuestReading {
        policy: GuestReadingPolicy::default(),
        revision: 0,
    };
    assert_eq!(
        repo.manageable_guest_reading(&owner_session, book)
            .await
            .unwrap(),
        Some(initial)
    );
    let prepared = GuestReadingPolicy::from_db("open", 10, 20).unwrap();
    for digest in [&translator_session, &outsider_session] {
        assert_eq!(
            repo.manageable_guest_reading(digest, book).await.unwrap(),
            None
        );
        assert_eq!(
            repo.replace_guest_reading(digest, book, 0, prepared).await,
            Err(AuthorError::Missing)
        );
    }
    assert_eq!(
        repo.replace_guest_reading(&[0; 32], book, 0, prepared)
            .await,
        Err(AuthorError::Forbidden)
    );
    assert_eq!(
        repo.replace_guest_reading(&owner_session, book, 0, initial.policy)
            .await,
        Ok(initial)
    );
    let restricted = GuestReadingPolicy::from_db("login_preview", 10, 20).unwrap();
    assert_eq!(
        repo.replace_guest_reading(&owner_session, book, 0, restricted)
            .await,
        Err(AuthorError::Forbidden)
    );
    let direct = sqlx::query("UPDATE books SET guest_read_mode='login_preview' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        direct.as_database_error().and_then(|db| db.constraint()),
        Some("books_guest_read_rollout_closed")
    );
    for sql in [
        "UPDATE books SET guest_open_through_chapter_no=-1 WHERE id=$1",
        "UPDATE books SET guest_preview_percent=15 WHERE id=$1",
        "UPDATE books SET guest_policy_revision=-1 WHERE id=$1",
        "UPDATE books SET guest_read_mode='unknown' WHERE id=$1",
    ] {
        assert!(sqlx::query(sql).bind(book).execute(&pool).await.is_err());
    }
    let first = repo
        .replace_guest_reading(&owner_session, book, 0, prepared)
        .await
        .unwrap();
    assert_eq!(first.revision, 1);
    let alternative = GuestReadingPolicy::from_db("open", 30, 30).unwrap();
    let other = GuestReadingPolicy::from_db("open", 20, 50).unwrap();
    let (left, right) = tokio::join!(
        repo.replace_guest_reading(&owner_session, book, 1, alternative),
        repo.replace_guest_reading(&owner_session, book, 1, other),
    );
    let winner = match (left, right) {
        (Ok(winner), Err(AuthorError::Conflict)) | (Err(AuthorError::Conflict), Ok(winner)) => winner,
        result => panic!("expected exactly one CAS winner: {result:?}"),
    };
    assert_eq!(winner.revision, 2);
    assert_eq!(
        repo.replace_guest_reading(&owner_session, book, 2, winner.policy)
            .await,
        Ok(winner)
    );
    let receipts: Vec<(i64, i64, i32, i32, i16, i16)> = sqlx::query_as(
        "SELECT revision,actor_id,old_open_through,new_open_through,
                old_preview_percent,new_preview_percent
         FROM book_guest_read_policy_events WHERE book_id=$1 ORDER BY revision",
    )
    .bind(book)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        receipts,
        vec![
            (1, owner, 0, 10, 20, 20),
            (
                2,
                owner,
                10,
                winner.policy.open_through(),
                20,
                winner.policy.preview_percent(),
            ),
        ]
    );
    assert!(sqlx::query(
        "UPDATE book_guest_read_policy_events SET new_open_through=0 WHERE book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .is_err());
    let unchanged: (String, String, String, i64) = sqlx::query_as(
        "SELECT visibility,access_level,guest_read_mode,guest_policy_revision
         FROM books WHERE id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged, ("draft".into(), "basic".into(), "open".into(), 2));
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.replace_guest_reading(&owner_session, book, 2, prepared)
            .await,
        Err(AuthorError::Missing)
    );
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(owner_session.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.replace_guest_reading(&owner_session, book, 2, prepared)
            .await,
        Err(AuthorError::Forbidden)
    );
}
