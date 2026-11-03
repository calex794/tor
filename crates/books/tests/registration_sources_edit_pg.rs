//! Owner link settings: isolated database, atomicity, permissions and races.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{new_book, AuthorError},
        book_registration::{
            source_identity, with_extra_source_links, with_registration_metadata,
            RegistrationFields,
        },
    },
};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s019_owner_can_edit_original_and_mirrors_without_claiming_other_books() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never use permanent PostgreSQL");
    let pool = PgPool::connect(&url).await.expect("PG");
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S019 editor','$argon2id$test') RETURNING id",
    )
    .bind(format!("s019-link-edit-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S019 outside','$argon2id$test') RETURNING id",
    )
    .bind(format!("s019-link-outside-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());

    let original = format!("https://original.example.cn/novel/{marker}");
    let updated_original = format!("https://edited.example.cn/story/{marker}");
    let mirror = format!("https://mirror.example.org/book/{marker}");
    let second_mirror = format!("https://other.example.net/work/{marker}");
    let rulate_id = marker % 800000000 + 100000;
    let rulate = format!("https://tl.rulate.ru/book/{rulate_id}?from=author");
    let rulate_variant = format!("http://www.rulate.ru/book/000{rulate_id}/?utm=copy");
    let key = "p".repeat(43);
    let identity = source_identity(&original, owner, &key).unwrap();
    let book = new_book(
        &identity,
        "ru",
        &format!("Первая книга редактирования {marker}"),
        &key,
    )
    .unwrap();
    let book = with_registration_metadata(
        book,
        RegistrationFields {
            source_url: &original,
            original_title: "",
            original_language: "zh",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    let book = with_extra_source_links(book, [&mirror, &rulate]).unwrap();
    let id = author.create_owned_book(owner, &book).await.unwrap();

    let read = author
        .owned_book_source_links(owner, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.primary, original);
    assert_eq!(read.extra.len(), 2);
    assert_eq!(read.revision, 1);
    assert!(author
        .owned_book_source_links(outsider, id)
        .await
        .unwrap()
        .is_none());
    assert!(author
        .owned_book_source_links(owner, -1)
        .await
        .unwrap()
        .is_none());

    assert_eq!(
        author
            .update_book_source_links(outsider, id, 1, [&updated_original, "", ""])
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author
            .update_book_source_links(
                owner,
                id,
                1,
                [&updated_original, &second_mirror, &rulate_variant],
            )
            .await,
        Ok(2)
    );
    let after = author
        .owned_book_source_links(owner, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.primary, updated_original);
    assert_eq!(after.revision, 2);
    assert_eq!(after.extra.len(), 2);
    assert!(after.extra.contains(&second_mirror));
    assert!(after.extra.contains(&rulate_variant));
    assert_eq!(
        author
            .update_book_source_links(owner, id, 1, [&original, "", ""])
            .await,
        Err(AuthorError::Conflict),
        "stale forms must not overwrite current source links"
    );

    assert_eq!(
        author
            .update_book_source_links(owner, id, 2, ["http://127.0.0.1/private", "", ""])
            .await,
        Err(AuthorError::Invalid)
    );
    let preserved: String = sqlx::query_scalar("SELECT source_identity FROM books WHERE id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        preserved, identity,
        "stable book identity must never be rewritten"
    );

    let other_key = "q".repeat(43);
    let other_url = format!("https://owned-by-another.example.org/story/{marker}");
    let other_identity = source_identity(&other_url, outsider, &other_key).unwrap();
    let other_book = new_book(
        &other_identity,
        "ru",
        &format!("Чужая книга {marker}"),
        &other_key,
    )
    .unwrap();
    let other_book = with_registration_metadata(
        other_book,
        RegistrationFields {
            source_url: &other_url,
            original_title: "",
            original_language: "en",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    let other_id = author
        .create_owned_book(outsider, &other_book)
        .await
        .unwrap();

    assert_eq!(
        author
            .update_book_source_links(owner, id, 2, [&updated_original, &other_url, ""])
            .await,
        Err(AuthorError::Conflict),
        "a link claimed by another owner must not be stolen"
    );
    let after_failed = author
        .owned_book_source_links(owner, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_failed.revision, 2);
    assert_eq!(
        after_failed.extra.len(),
        2,
        "conflict rolls back every claim"
    );
    assert_eq!(after_failed.primary, updated_original);

    let own_links: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_registration_links WHERE book_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(own_links, 3);
    assert_ne!(id, other_id);

    // A removed extra URL is no longer claimed, but the book keeps its
    // original identity, chapters, ownership and publication state.
    assert_eq!(
        author
            .update_book_source_links(owner, id, 2, [&updated_original, "", ""])
            .await,
        Ok(3)
    );
    let links_after_remove: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_registration_links WHERE book_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(links_after_remove, 1);
    let new_key = "r".repeat(43);
    let reuse = new_book(
        &source_identity(&second_mirror, outsider, &new_key).unwrap(),
        "ru",
        &format!("Другая книга по освобождённой ссылке {marker}"),
        &new_key,
    )
    .unwrap();
    let reuse = with_registration_metadata(
        reuse,
        RegistrationFields {
            source_url: &second_mirror,
            original_title: "",
            original_language: "zh",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    assert!(
        author.create_owned_book(outsider, &reuse).await.is_ok(),
        "removing an optional mirror should release its claim"
    );

    // Existing historical book has no link claims; exact URLs still block edits.
    let historical = format!("https://old.example.cn/novel/{marker}");
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,source_url)
         VALUES($1,'ru',$2,$3)",
    )
    .bind(format!("historical:links:{marker}"))
    .bind(format!("Историческая книга {marker}"))
    .bind(&historical)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        author
            .update_book_source_links(owner, id, 3, [&updated_original, &historical, ""])
            .await,
        Err(AuthorError::Conflict),
        "legacy link collision must not change existing book"
    );

    // A historical source link is protected by canonical URL, not a Rulate work ID.
    let another_historical = format!("https://tl.rulate.ru/book/{}?ref=old", rulate_id + 1);
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,source_url)
         VALUES($1,'ru',$2,$3)",
    )
    .bind(format!("historical:rulate-edit:{marker}"))
    .bind(format!("Историческая ссылка Rulate {marker}"))
    .bind(&another_historical)
    .execute(&pool)
    .await
    .unwrap();
    let alias = format!("http://www.tl.rulate.ru/book/{}/?ref=old", rulate_id + 1);
    assert_eq!(
        author
            .update_book_source_links(owner, id, 3, [&updated_original, &alias, ""])
            .await,
        Err(AuthorError::Conflict),
        "historical canonical links from any site remain protected"
    );
    pool.close().await;
}
