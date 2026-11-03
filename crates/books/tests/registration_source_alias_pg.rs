//! S019: no duplicate book under a different name or provider URL.
use sqlx::{PgPool, Row};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::{
        authoring::{new_book, AuthorError},
        book_registration::{
            rulate_work_id, source_identity, with_registration_metadata, RegistrationFields,
        },
    },
};

async fn fixture() -> (PgPool, i64, u128) {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Disposable CI PostgreSQL only");
    let pool = PgPool::connect(&url).await.expect("connect");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized, display_name, password_hash)
         VALUES($1, 'S019 alias author', '$argon2id$test') RETURNING id",
    )
    .bind(format!("s019-alias-{nonce}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    (pool, user, nonce)
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s019_historical_original_and_alternative_titles_block_new_claims() {
    let (pool, owner, nonce) = fixture().await;
    let old_original = format!("Original Work {nonce}");
    let old_alt = format!("Еж и кот {nonce}");
    let old_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity, target_language, title, visibility,
                           owner_user_id, original_title, alt_titles)
         VALUES($1, 'ru', $2, 'draft', $3, $4, $5) RETURNING id",
    )
    .bind(format!("legacy:aliases:{nonce}"))
    .bind(format!("Приватный старый черновик {nonce}"))
    .bind(owner)
    .bind(&old_original)
    .bind(vec![old_alt.clone()])
    .fetch_one(&pool)
    .await
    .unwrap();

    let repo = PgAuthoring::new(pool.clone());
    let key = "a".repeat(43);
    let book = new_book(
        &format!("manual:{owner}:{key}"),
        "ru",
        &format!("Совершенно другое название {nonce}"),
        &key,
    )
    .unwrap();
    let book = with_registration_metadata(
        book,
        RegistrationFields {
            source_url: "",
            original_title: "",
            original_language: "en",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: [&old_alt.replace("Еж", "ЁЖ"), "", ""],
        },
    )
    .unwrap();
    assert_eq!(
        repo.create_owned_book(owner, &book).await,
        Err(AuthorError::Conflict),
        "legacy alternate title must block a new registration"
    );

    let key2 = "b".repeat(43);
    let second = new_book(
        &format!("manual:{owner}:{key2}"),
        "ru",
        &format!("Другой перевод {nonce}"),
        &key2,
    )
    .unwrap();
    let second = with_registration_metadata(
        second,
        RegistrationFields {
            source_url: "",
            original_title: &old_original,
            original_language: "en",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    assert_eq!(
        repo.create_owned_book(owner, &second).await,
        Err(AuthorError::Conflict),
        "legacy original title must block a new registration"
    );
    let claimed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_registration_titles WHERE book_id=$1")
            .bind(old_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(claimed, 0, "historical drafts must remain unchanged");
    let visibility: String = sqlx::query_scalar("SELECT visibility FROM books WHERE id=$1")
        .bind(old_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(visibility, "draft");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s019_known_provider_work_id_links_block_different_titles() {
    let (pool, owner, nonce) = fixture().await;
    let number = nonce % 9_000_000_000 + 100_000;
    let old_link = format!("http://tl.rulate.ru/book/000{number}/?utm_source=legacy");
    let new_link = format!("https://www.rulate.ru/book/{number}?utm_campaign=sharing");
    assert_eq!(rulate_work_id(&old_link), rulate_work_id(&new_link));
    let db_key: Option<String> = sqlx::query_scalar("SELECT book_registration_rulate_work_id($1)")
        .bind(&old_link)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db_key, rulate_work_id(&new_link));

    sqlx::query(
        "INSERT INTO books(source_identity, target_language, title, source_url)
         VALUES($1, 'ru', $2, $3)",
    )
    .bind(format!("legacy:rulate:{nonce}"))
    .bind(format!("Историческая версия {nonce}"))
    .bind(&old_link)
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgAuthoring::new(pool.clone());
    let key = "c".repeat(43);
    let identity = source_identity(&new_link, owner, &key).unwrap();
    let proposed = new_book(&identity, "ru", &format!("Совсем другое имя {nonce}"), &key).unwrap();
    let proposed = with_registration_metadata(
        proposed,
        RegistrationFields {
            source_url: &new_link,
            original_title: "",
            original_language: "zh",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    assert_eq!(
        repo.create_owned_book(owner, &proposed).await,
        Err(AuthorError::Conflict),
        "provider work number must defeat changed titles and URL tracking"
    );

    let key2 = "d".repeat(43);
    let in_english = new_book(
        &source_identity(&new_link, owner, &key2).unwrap(),
        "en",
        &format!("Independent edition {nonce}"),
        &key2,
    )
    .unwrap();
    let in_english = with_registration_metadata(
        in_english,
        RegistrationFields {
            source_url: &new_link,
            original_title: "",
            original_language: "zh",
            release_year: "",
            project_status: "",
            catalog_section: "",
            alt_titles: ["", "", ""],
        },
    )
    .unwrap();
    let english_id = repo.create_owned_book(owner, &in_english).await.unwrap();
    let english_row = sqlx::query("SELECT visibility, source_identity FROM books WHERE id=$1")
        .bind(english_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(english_row.get::<String, _>("visibility"), "draft");
    assert!(english_row
        .get::<String, _>("source_identity")
        .starts_with("rulate:book:"));
    let key3 = "e".repeat(43);
    let another_url = format!("https://tl.rulate.ru/book/{number}/?ref=other");
    let duplicate = new_book(
        &source_identity(&another_url, owner, &key3).unwrap(),
        "en",
        &format!("Alternative English title {nonce}"),
        &key3,
    )
    .unwrap();
    assert_eq!(
        repo.create_owned_book(owner, &duplicate).await,
        Err(AuthorError::Conflict),
        "new versions are also protected by the existing unique source identity"
    );
    pool.close().await;
}
