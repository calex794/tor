//! B-19: actual PostgreSQL 18 duplicate hints before registering a book.
use sqlx::{PgPool, Row};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{new_book, AuthorError},
        title_duplicates::{candidate_titles, with_title_aliases},
    },
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_19_title_suggestions_are_indexed_private_and_non_blocking() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never mutate production");
    let pool = PgPool::connect(&url).await.expect("PG connect");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut actors = Vec::new();
    for index in 0..2 {
        let user: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES ($1,'B19 fixture','$argon2id$test') RETURNING id",
        )
        .bind(format!("b19-{stamp}-{index}@tor-ci.example"))
        .fetch_one(&pool)
        .await
        .unwrap();
        actors.push(user);
    }
    let (owner, other) = (actors[0], actors[1]);
    let authors = PgAuthoring::new(pool.clone());
    let public = with_title_aliases(
        new_book(
            &format!("b19:{stamp}:public"),
            "ru",
            "Синяя фиалка",
            &"a".repeat(43),
        )
        .unwrap(),
        "Blue Violet",
        "Голубая фиалка",
    )
    .unwrap();
    let id = authors.create_owned_book(owner, &public).await.unwrap();
    assert_eq!(authors.create_owned_book(owner, &public).await, Ok(id));
    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let alias_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM book_title_aliases WHERE book_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(alias_count, 3);
    for (name, is_exact) in [
        ("СИНЯЯ ФИАЛКА", true),
        ("Blue Violet", true),
        ("Голубая фиалка", true),
        ("Синяя фиалк", false),
    ] {
        let matches = authors
            .title_candidates(other, &[name.to_owned()])
            .await
            .unwrap();
        let hit = matches.iter().find(|candidate| candidate.id == id).unwrap();
        assert_eq!(hit.exact, is_exact, "{name}");
    }
    let hidden = new_book(
        &format!("b19:{stamp}:hidden"),
        "ru",
        "Секретное ожерелье",
        &"b".repeat(43),
    )
    .unwrap();
    let hidden_id = authors.create_owned_book(owner, &hidden).await.unwrap();
    let private = candidate_titles("Секретное ожерелье", "", "").unwrap();
    assert!(authors
        .title_candidates(other, &private)
        .await
        .unwrap()
        .iter()
        .all(|match_| match_.id != hidden_id));
    assert!(authors
        .title_candidates(owner, &private)
        .await
        .unwrap()
        .iter()
        .any(|match_| match_.id == hidden_id));
    assert_eq!(
        authors.title_candidates(0, &private).await.err(),
        Some(AuthorError::Forbidden)
    );

    // Same title is NOT a canonical identity: unrelated books may share it.
    let different = new_book(
        &format!("b19:{stamp}:different-work"),
        "en",
        "Синяя фиалка",
        &"c".repeat(43),
    )
    .unwrap();
    assert!(authors.create_owned_book(other, &different).await.is_ok());

    let row = sqlx::query("SELECT source_identity, target_language FROM books WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("target_language"), "ru");
    assert!(row.get::<String, _>("source_identity").contains("b19:"));
    pool.close().await;
}
