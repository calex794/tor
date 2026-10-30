//! E-05: real PostgreSQL periods and untrusted source-book rejection.
//! Requires disposable tor_ci, never a deployed server.
use sqlx::PgPool;
use tor_social::services::follow::{CreatorFollowError, PgCreatorFollows};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending with /tor_ci"]
async fn e_05_pg_creator_follow_unfollow_refollow_source_and_no_entitlement() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable test DB");
    assert!(url.ends_with("/tor_ci"), "production DB forbidden");
    let pool = PgPool::connect(&url).await.expect("connect");
    for (id, email, name) in [
        (825001_i64, "e05-a@example.invalid", "Автор <A>"),
        (825002, "e05-b@example.invalid", "Читатель"),
        (825003, "e05-c@example.invalid", "Другой автор"),
    ] {
        sqlx::query(
            "INSERT INTO users(id,email_normalized,display_name,password_hash)
             OVERRIDING SYSTEM VALUE VALUES ($1,$2,$3,'$argon2id$fixture')",
        )
        .bind(id)
        .bind(email)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility,owner_user_id)
         VALUES
          (825101,'ci:e05:public','ru','Книга <script>','public',825001),
          (825102,'ci:e05:draft','ru','Скрытая книга','draft',825003)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgCreatorFollows::new(pool.clone());
    assert_eq!(repo.creator_for_public_book(825101).await.unwrap(), Some(825001));
    assert_eq!(repo.creator_for_public_book(825102).await.unwrap(), None);
    assert_eq!(
        repo.follow_from_book(825002,825102).await,
        Err(CreatorFollowError::NotFound)
    );
    assert_eq!(
        repo.follow_from_book(825001,825101).await,
        Err(CreatorFollowError::Forbidden)
    );
    assert_eq!(repo.follow_from_book(825002,825101).await.unwrap(), 825001);
    let first: (i64,String) = sqlx::query_as(
        "SELECT id,started_at::text FROM creator_follow_periods
         WHERE follower_user_id=825002 AND creator_user_id=825001 AND ended_at IS NULL",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(repo.follow_from_book(825002,825101).await.unwrap(), 825001);
    let same: (i64,String) = sqlx::query_as(
        "SELECT id,started_at::text FROM creator_follow_periods
         WHERE follower_user_id=825002 AND creator_user_id=825001 AND ended_at IS NULL",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(first,same,"repeat follow must not reset 90-day tenure");
    assert!(repo.is_following(825002,825001).await.unwrap());
    let visible=repo.list_active(825002,None).await.unwrap();
    assert_eq!(visible.len(),1);
    assert_eq!(visible[0].creator_user_id,825001);
    assert_eq!(visible[0].source_book,Some((825101,"Книга <script>".to_owned())));
    assert_eq!(repo.list_active(825003,None).await.unwrap().len(),0);
    assert_eq!(repo.unfollow(825002,825001).await,Ok(()));
    assert_eq!(repo.unfollow(825002,825001).await,Ok(()));
    assert!(!repo.is_following(825002,825001).await.unwrap());
    assert!(repo.list_active(825002,None).await.unwrap().is_empty());
    assert_eq!(repo.follow_from_book(825002,825101).await.unwrap(),825001);
    let second: i64=sqlx::query_scalar(
        "SELECT id FROM creator_follow_periods
         WHERE follower_user_id=825002 AND creator_user_id=825001 AND ended_at IS NULL",
    ).fetch_one(&pool).await.unwrap();
    assert_ne!(first.0,second,"new period resets 90-day tenure");
    let closed: i64=sqlx::query_scalar(
        "SELECT count(*) FROM creator_follow_periods
         WHERE follower_user_id=825002 AND creator_user_id=825001 AND ended_at IS NOT NULL",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(closed,1);
    let book_follows: i64=sqlx::query_scalar(
        "SELECT count(*) FROM reader_book_follows WHERE user_id=825002",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(book_follows,0,"creator follow must not become a book follow");
    let purchases: i64=sqlx::query_scalar(
        "SELECT count(*) FROM chapter_purchase_facts WHERE user_id=825002",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(purchases,0,"creator follow must never grant paid access");
    let source: (i64,String)=sqlx::query_as(
        "SELECT source_book_id,source_kind FROM creator_follow_periods WHERE id=$1",
    ).bind(second).fetch_one(&pool).await.unwrap();
    assert_eq!(source,(825101,"book_page".to_owned()));

    // Book becomes private: the subscriber can still unsubscribe, but the
    // source metadata must never appear in the subscriber's listing.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=825101")
        .execute(&pool).await.unwrap();
    let private=repo.list_active(825002,None).await.unwrap();
    assert!(private[0].source_book.is_none());
    assert_eq!(repo.unfollow(825002,825001).await,Ok(()));

    sqlx::query("DELETE FROM books WHERE id IN (825101,825102)")
        .execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id IN (825001,825002,825003)")
        .execute(&pool).await.unwrap();
}
