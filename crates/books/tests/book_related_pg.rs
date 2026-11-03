//! S020: reciprocal related-book creation, duplicate-safe concurrent adds,
//! owner checks and private-target non-disclosure on real disposable PG.
use sqlx::PgPool;
use tor_books::repositories::postgres::PgAuthoring;
use tor_books::services::authoring::AuthorError;

#[tokio::test]
#[ignore = "requires local isolated /tor_ci PostgreSQL"]
async fn s020_related_books_are_bilateral_and_never_expose_private_targets() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let pool = PgPool::connect(&url).await.unwrap();
    let mark = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Related Author','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-related-owner-{mark}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let another: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Related Other','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-related-other-{mark}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();

    async fn make_book(
        pool: &PgPool,
        identity: String,
        title: String,
        owner: i64,
        visibility: &str,
        held: bool,
    ) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                               visibility,access_level,admin_hold)
             VALUES($1,'ru',$2,$3,$4,'basic',$5) RETURNING id",
        )
        .bind(identity)
        .bind(title)
        .bind(owner)
        .bind(visibility)
        .bind(held)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    let a_title = format!("S020 Связанное произведение {mark}");
    let b_title = format!("S020 Продолжение {mark}");
    let private_title = format!("S020 Скрытое продолжение {mark}");
    let held_title = format!("S020 Заблокированное {mark}");
    let a = make_book(
        &pool,
        format!("related:a:{mark}"),
        a_title.clone(),
        owner,
        "public",
        false,
    )
    .await;
    let b = make_book(
        &pool,
        format!("related:b:{mark}"),
        b_title.clone(),
        another,
        "public",
        false,
    )
    .await;
    let hidden = make_book(
        &pool,
        format!("related:private:{mark}"),
        private_title.clone(),
        another,
        "draft",
        false,
    )
    .await;
    let held = make_book(
        &pool,
        format!("related:held:{mark}"),
        held_title.clone(),
        another,
        "public",
        true,
    )
    .await;
    let repo = PgAuthoring::new(pool.clone());

    assert!(repo
        .owned_related_books(another, a)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.find_related_candidates(another, a, &b_title).await,
        Err(AuthorError::Missing)
    );
    let found = repo
        .find_related_candidates(owner, a, &b_title.to_uppercase())
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, b);
    assert!(repo
        .find_related_candidates(owner, a, &private_title)
        .await
        .unwrap()
        .is_empty());
    assert!(repo
        .find_related_candidates(owner, a, &held_title)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo.add_related_book(owner, a, hidden).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(owner, a, held).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(another, a, b).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        repo.add_related_book(owner, a, a).await,
        Err(AuthorError::Invalid)
    );
    // Two writers linking opposite directions concurrently cannot create
    // duplicate rows or wedge on opposing row locks.
    let (left, right) = tokio::join!(
        repo.add_related_book(owner, a, b),
        repo.add_related_book(another, b, a)
    );
    assert_eq!(left, Ok(()));
    assert_eq!(right, Ok(()));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM book_related_links
         WHERE smaller_book_id=$1 AND larger_book_id=$2",
    )
    .bind(a.min(b))
    .bind(a.max(b))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "one canonical row is bilateral");
    let a_links = repo.owned_related_books(owner, a).await.unwrap().unwrap();
    let b_links = repo.owned_related_books(another, b).await.unwrap().unwrap();
    assert_eq!(a_links.linked.len(), 1);
    assert_eq!(a_links.linked[0].id, b);
    assert_eq!(b_links.linked.len(), 1);
    assert_eq!(b_links.linked[0].id, a);

    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .owned_related_books(owner, a)
        .await
        .unwrap()
        .unwrap()
        .linked
        .is_empty());
    assert_eq!(
        repo.remove_related_book(another, a, b).await,
        Err(AuthorError::Missing)
    );
    // The owner can remove a formerly public link after the target hides.
    assert_eq!(repo.remove_related_book(owner, a, b).await, Ok(true));
    assert_eq!(repo.remove_related_book(owner, a, b).await, Ok(false));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_related_links WHERE smaller_book_id=$1")
            .bind(a.min(b))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    pool.close().await;
}
