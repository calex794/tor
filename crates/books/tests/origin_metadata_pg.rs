//! S020: per-book active membership for origin metadata, optimistic revision, visibility isolation.
use sqlx::PgPool;
use tor_books::repositories::{book_members::PgBookMembers, postgres::{OriginFields, PgAuthoring}};
use tor_books::services::authoring::{AuthorError, DiscoveryFields};
use tor_contracts::access::BookRole;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_book_member_origin_metadata_preserves_access_and_publication() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let marker = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Origin owner','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-origin-owner-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Origin other','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-origin-other-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           visibility,access_level,original_language,release_year,catalog_section)
         VALUES($1,'ru',$2,$3,'draft','basic','zh',2022,'novels/chinese')
         RETURNING id",
    )
    .bind(format!("s020:origin:{marker}"))
    .bind(format!("Оригинал книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let books = PgAuthoring::new(pool.clone());
    let initial = books
        .manageable_origin_metadata(owner, book_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.original_language, "zh");
    assert_eq!(initial.release_year, Some(2022));
    assert_eq!(initial.revision, 1);
    assert!(books
        .manageable_origin_metadata(outsider, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_origin_metadata(outsider, book_id, 1, "en", "2025", "novels/english")
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        books
            .update_origin_metadata(owner, book_id, 1, "EN", "2025", "novels/english")
            .await,
        Ok(2)
    );
    assert_eq!(
        books
            .update_origin_metadata(owner, book_id, 1, "zh", "2024", "novels/chinese")
            .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(
        books
            .update_origin_metadata(owner, book_id, 2, "zh", "invalid", "")
            .await,
        Err(AuthorError::Invalid)
    );
    let (language, year, section, revision, visibility, access): (
        String,
        Option<i32>,
        String,
        i64,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT original_language,release_year,catalog_section,
                origin_metadata_revision,visibility,access_level
         FROM books WHERE id=$1",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(language, "en");
    assert_eq!(year, Some(2025));
    assert_eq!(section, "novels/english");
    assert_eq!(revision, 2);
    assert_eq!(visibility, "draft");
    assert_eq!(access, "basic");

    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Origin translator','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-origin-translator-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let moderator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Origin moderator','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-origin-moderator-{marker}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let members = PgBookMembers::new(pool.clone());
    members
        .add_collaborator(owner, book_id, translator, BookRole::Translator)
        .await
        .unwrap();
    members
        .add_collaborator(owner, book_id, moderator, BookRole::Moderator)
        .await
        .unwrap();
    assert!(books
        .manageable_origin_metadata(translator, book_id)
        .await
        .unwrap()
        .is_some());
    assert!(books
        .manageable_origin_metadata(moderator, book_id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        books
            .update_origin_metadata(translator, book_id, 2, "ja", "2023", "novels/japanese")
            .await,
        Ok(3)
    );
    assert_eq!(
        books
            .update_origin_metadata(moderator, book_id, 3, "ko", "2024", "novels/korean")
            .await,
        Ok(4)
    );
    assert_eq!(
        books
            .update_origin_metadata(translator, book_id, 2, "en", "2021", "novels/english")
            .await,
        Err(AuthorError::Conflict)
    );
    let unrelated: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id)
         VALUES($1,'ru',$2,$3) RETURNING id",
    )
    .bind(format!("s020:unrelated-origin:{marker}"))
    .bind(format!("Unrelated {marker}"))
    .bind(outsider)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(books
        .manageable_origin_metadata(translator, unrelated)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_origin_metadata(translator, unrelated, 1, "zh", "2024", "")
            .await,
        Err(AuthorError::Missing)
    );
    members
        .revoke_collaborator(owner, book_id, moderator)
        .await
        .unwrap();
    assert!(books
        .manageable_origin_metadata(moderator, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_origin_metadata(moderator, book_id, 4, "zh", "2024", "")
            .await,
        Err(AuthorError::Missing)
    );
    assert!(books
        .manageable_origin_metadata(translator, book_id)
        .await
        .unwrap()
        .is_some());
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(books
        .manageable_origin_metadata(owner, book_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        books
            .update_origin_metadata(translator, book_id, 4, "zh", "2024", "")
            .await,
        Err(AuthorError::Missing)
    );
    sqlx::query("UPDATE books SET admin_hold=FALSE, is_deleted=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(books
        .manageable_origin_metadata(translator, book_id)
        .await
        .unwrap()
        .is_none());

    // One full S020 settings submission writes discovery fields and origin
    // metadata in the same optimistic transaction. Old author/source, paid
    // and publication invariants remain outside this edit.
    let discovery_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,
                           visibility,access_level,author_name,genres,tags,fandoms)
         VALUES($1,'ru',$2,$3,'draft','basic','Старый автор',
                ARRAY['фэнтези']::TEXT[],ARRAY[]::TEXT[],ARRAY[]::TEXT[])
         RETURNING id",
    )
    .bind(format!("s020:discovery:{marker}"))
    .bind(format!("Темы книги {marker}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let start = books
        .manageable_origin_metadata(owner, discovery_book)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(start.author_name, "Старый автор");
    assert_eq!(start.genres, vec!["фэнтези"]);
    assert_eq!(
        books
            .update_origin_and_discovery(
                owner,
                discovery_book,
                start.revision,
                OriginFields {
                    language: "ZH",
                    year: "2024",
                    section: "novels/chinese",
                },
                DiscoveryFields {
                    author_name: "  Новый автор ",
                    genres: "Фэнтези, фантастика, фэнтези",
                    tags: "Магия, Романтика",
                    fandoms: "Наруто",
                },
            )
            .await,
        Ok(start.revision + 1),
    );
    let saved = books
        .manageable_origin_metadata(owner, discovery_book)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.author_name, "Новый автор");
    assert_eq!(saved.genres, vec!["фэнтези", "фантастика"]);
    assert_eq!(saved.tags, vec!["магия", "романтика"]);
    assert_eq!(saved.fandoms, vec!["наруто"]);
    assert_eq!(saved.original_language, "zh");
    assert_eq!(saved.release_year, Some(2024));
    assert_eq!(saved.catalog_section, "novels/chinese");
    assert_eq!(
        books
            .update_origin_and_discovery(
                owner,
                discovery_book,
                start.revision,
                OriginFields {
                    language: "en",
                    year: "2023",
                    section: "novels/english",
                },
                DiscoveryFields {
                    author_name: "stale",
                    genres: "",
                    tags: "",
                    fandoms: "",
                },
            )
            .await,
        Err(AuthorError::Conflict),
    );
    assert_eq!(
        books
            .update_origin_and_discovery(
                outsider,
                discovery_book,
                saved.revision,
                OriginFields {
                    language: "en",
                    year: "2023",
                    section: "novels/english",
                },
                DiscoveryFields {
                    author_name: "forged",
                    genres: "",
                    tags: "",
                    fandoms: "",
                },
            )
            .await,
        Err(AuthorError::Missing),
    );
    let (visibility, access, author): (String, String, String) =
        sqlx::query_as("SELECT visibility,access_level,author_name FROM books WHERE id=$1")
            .bind(discovery_book)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (visibility.as_str(), access.as_str(), author.as_str()),
        ("draft", "basic", "Новый автор")
    );

    pool.close().await;
}
