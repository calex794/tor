//! S019: one Chinese original can carry arbitrary extra translation links.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{new_book, AuthorError, NewBook},
        book_registration::{
            source_identity, with_extra_source_links, with_registration_metadata,
            RegistrationFields,
        },
    },
};

fn request(owner: i64, key: &str, language: &str, title: &str, original: &str, other: [&str; 2]) -> NewBook {
    let identity = source_identity(original, owner, key).unwrap();
    let book = new_book(&identity, language, title, key).unwrap();
    let book = with_registration_metadata(
        book,
        RegistrationFields {
            source_url: original,
            original_title: "",
            original_language: "zh",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    with_extra_source_links(book, other).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s019_multiple_links_are_atomic_and_language_scoped() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "CI database only");
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES($1, 'S019 links author', '$argon2id$test') RETURNING id",
    )
    .bind(format!("s019-links-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let original = format!("https://original.example.cn/novel/{marker}");
    let mirror = format!("https://mirror.example.org/work/{marker}");
    let rulate_id = marker % 8_000_000_000 + 100000;
    let rulate = format!("https://tl.rulate.ru/book/{rulate_id}?utm_source=author");
    let rulate_alternate = format!("http://www.rulate.ru/book/000{rulate_id}/?ref=other");
    let repo = PgAuthoring::new(pool.clone());

    let key = "a".repeat(43);
    let first = request(
        owner,
        &key,
        "ru",
        &format!("Книга с первоисточником {marker}"),
        &original,
        [&mirror, &rulate],
    );
    let book_id = repo.create_owned_book(owner, &first).await.unwrap();
    assert_eq!(
        repo.create_owned_book(owner, &first).await.unwrap(),
        book_id,
        "same command must return original book"
    );
    let links: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_registration_links WHERE book_id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(links, 3, "original and both optional links are indexed");

    let changed_key = request(
        owner,
        &key,
        "ru",
        &format!("Книга с первоисточником {marker}"),
        &original,
        [&mirror, ""],
    );
    assert_eq!(
        repo.create_owned_book(owner, &changed_key).await,
        Err(AuthorError::Conflict),
        "a changed source-link payload cannot reuse the command"
    );

    let key2 = "b".repeat(43);
    let another_original = format!("https://elsewhere.example.net/novel/{marker}");
    let by_mirror = request(
        owner,
        &key2,
        "ru",
        &format!("Другое название зеркала {marker}"),
        &another_original,
        [&mirror, ""],
    );
    assert_eq!(
        repo.create_owned_book(owner, &by_mirror).await,
        Err(AuthorError::Conflict),
        "same mirror URL under a different title cannot create a second book"
    );
    let key3 = "c".repeat(43);
    let by_provider = request(
        owner,
        &key3,
        "ru",
        &format!("Другое название перевода {marker}"),
        &another_original,
        [&rulate_alternate, ""],
    );
    assert_eq!(
        repo.create_owned_book(owner, &by_provider).await,
        Err(AuthorError::Conflict),
        "Rulate variant is covered even when it is an extra URL"
    );

    let key4 = "d".repeat(43);
    let in_english = request(
        owner,
        &key4,
        "en",
        &format!("Other language {marker}"),
        &original,
        [&mirror, &rulate],
    );
    assert!(
        repo.create_owned_book(owner, &in_english).await.is_ok(),
        "different translation languages remain independent"
    );

    let old_url = format!("https://old-source.example.cn/work/{marker}");
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,source_url)
         VALUES($1,'ru',$2,$3)",
    )
    .bind(format!("legacy:source-links:{marker}"))
    .bind(format!("Старое произведение {marker}"))
    .bind(&old_url)
    .execute(&pool)
    .await
    .unwrap();
    let key5 = "e".repeat(43);
    let old_link = request(
        owner,
        &key5,
        "ru",
        &format!("Другая регистрация старой ссылки {marker}"),
        &another_original,
        [&old_url, ""],
    );
    assert_eq!(
        repo.create_owned_book(owner, &old_link).await,
        Err(AuthorError::Conflict),
        "exact legacy source URL must be checked without rewriting old books"
    );
    pool.close().await;
}
