//! B/TAX-01: canonical server-side selection and persistence on disposable PG.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError, TaxonomySelection},
};

#[tokio::test]
#[ignore = "requires disposable tor_ci app and migrator database URLs"]
async fn indexed_taxonomy_lookup_and_id_only_creation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL URL");
    let admin_url = std::env::var("TEST_TAXONOMY_MIGRATOR_URL")
        .expect("disposable migration owner URL for curated fixture");
    assert!(url.ends_with("/tor_ci"), "Never mutate production");
    assert!(
        admin_url.ends_with("/tor_ci"),
        "Never seed production taxonomy"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let admin_pool = PgPool::connect(&admin_url).await.unwrap();
    let author = PgAuthoring::new(pool.clone());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Taxonomy Owner','$argon2id$test') RETURNING id",
    )
    .bind(format!("taxonomy-{stamp}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    // The runtime role must never be able to create arbitrary public tags.
    assert!(
        sqlx::query(
            "INSERT INTO book_taxonomy_terms(kind,label,search_key)
                     VALUES('tag','Unauthorized','unauthorized')"
        )
        .execute(&pool)
        .await
        .is_err(),
        "runtime role may link approved terms, not create them"
    );
    let genre_key = format!("fantasy{stamp}");
    let tag_key = format!("system{stamp}");
    let alias_key = format!("синоним{stamp}");
    let genre: i64 = sqlx::query_scalar(
        "INSERT INTO book_taxonomy_terms(kind,label,search_key)
         VALUES('genre',$1,$2) RETURNING id",
    )
    .bind("Fantasy")
    .bind(&genre_key)
    .fetch_one(&admin_pool)
    .await
    .unwrap();
    let tag: i64 = sqlx::query_scalar(
        "INSERT INTO book_taxonomy_terms(kind,label,search_key)
         VALUES('tag','System',$1) RETURNING id",
    )
    .bind(&tag_key)
    .fetch_one(&admin_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_taxonomy_aliases(kind,search_key,term_id)
         VALUES('tag',$1,$2)",
    )
    .bind(&alias_key)
    .bind(tag)
    .execute(&admin_pool)
    .await
    .unwrap();

    let by_name = author
        .suggest_taxonomies("genre", "fantasy", 15)
        .await
        .unwrap();
    assert!(by_name.iter().any(|row| row.id == genre));
    let by_alias = author
        .suggest_taxonomies("tag", "синоним", 15)
        .await
        .unwrap();
    assert!(by_alias.iter().any(|row| row.id == tag));
    assert_eq!(
        author.suggest_taxonomies("tag", "%", 15).await.unwrap_err(),
        AuthorError::Invalid,
    );
    assert_eq!(
        author
            .suggest_taxonomies("unknown", "system", 15)
            .await
            .unwrap_err(),
        AuthorError::Invalid,
    );

    let selected =
        TaxonomySelection::parse(Some(&genre.to_string()), Some(&tag.to_string()), None).unwrap();
    let names = author.resolve_taxonomy_selection(&selected).await.unwrap();
    // Compatibility arrays store normalized B labels, not display casing.
    assert_eq!(names.genres, vec![genre_key.clone()]);
    assert_eq!(names.tags, vec![tag_key.clone()]);

    let mut book = new_book(
        &format!("ci:taxonomy:{stamp}"),
        "ru",
        "Canonical taxonomy book",
        &"t".repeat(43),
    )
    .unwrap();
    book.genres = names.genres;
    book.tags = names.tags;
    book.fandoms = names.fandoms;
    book.taxonomy_ids = selected.clone();
    let book_id = author.create_owned_book(user, &book).await.unwrap();
    assert_eq!(author.create_owned_book(user, &book).await, Ok(book_id));
    let actual: (Vec<String>, Vec<String>) =
        sqlx::query_as("SELECT genres,tags FROM books WHERE id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actual, (vec![genre_key.clone()], vec![tag_key.clone()]));
    let links: Vec<i64> = sqlx::query_scalar(
        "SELECT term_id FROM book_taxonomy_links WHERE book_id=$1
         ORDER BY kind,position",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(links, vec![genre, tag]);

    // Existing books can edit their IDs, but cannot overwrite concurrent
    // changes or silently erase old metadata outside the canonical dictionary.
    let initial = author.taxonomy_snapshot(user, book_id).await.unwrap().unwrap();
    assert_eq!(initial.revision, 1);
    assert!(initial.editable);
    assert_eq!(initial.genres.iter().map(|t| t.id).collect::<Vec<_>>(), vec![genre]);
    assert_eq!(initial.tags.iter().map(|t| t.id).collect::<Vec<_>>(), vec![tag]);
    assert!(author.taxonomy_snapshot(user + 500, book_id).await.unwrap().is_none());
    assert_eq!(
        author.update_book_taxonomy(user + 500, book_id, 1, &selected).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author.update_book_taxonomy(user, book_id, 0, &selected).await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        author.update_book_taxonomy(user, book_id, 1, &selected).await,
        Ok(()),
        "repeating the same selections must not advance revision"
    );
    assert_eq!(author.taxonomy_snapshot(user, book_id).await.unwrap().unwrap().revision, 1);
    let genre_only = TaxonomySelection::parse(Some(&genre.to_string()), None, None).unwrap();
    assert_eq!(
        author.update_book_taxonomy(user, book_id, 1, &genre_only).await,
        Ok(())
    );
    let changed = author.taxonomy_snapshot(user, book_id).await.unwrap().unwrap();
    assert!(changed.editable);
    assert_eq!(changed.revision, 2);
    assert!(changed.tags.is_empty());
    assert_eq!(
        author.update_book_taxonomy(user, book_id, 1, &selected).await,
        Err(AuthorError::Conflict),
        "stale form cannot restore a removed tag"
    );
    let updated_tags: Vec<String> = sqlx::query_scalar("SELECT tags FROM books WHERE id=$1")
        .bind(book_id).fetch_one(&pool).await.unwrap();
    assert!(updated_tags.is_empty());
    let updated_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT term_id FROM book_taxonomy_links WHERE book_id=$1 ORDER BY kind,position",
    ).bind(book_id).fetch_all(&pool).await.unwrap();
    assert_eq!(updated_ids, vec![genre]);

    sqlx::query("UPDATE books SET tags=ARRAY['unmapped_legacy'] WHERE id=$1")
        .bind(book_id).execute(&pool).await.unwrap();
    let orphan = author.taxonomy_snapshot(user, book_id).await.unwrap().unwrap();
    assert!(!orphan.editable, "legacy private values must not be discarded");
    assert_eq!(orphan.raw_tags, vec!["unmapped_legacy"]);
    assert_eq!(
        author.update_book_taxonomy(user, book_id, 2, &genre_only).await,
        Err(AuthorError::Conflict)
    );
    let preserved: Vec<String> = sqlx::query_scalar("SELECT tags FROM books WHERE id=$1")
        .bind(book_id).fetch_one(&pool).await.unwrap();
    assert_eq!(preserved, vec!["unmapped_legacy"]);

    // Forged IDs never create free-text taxonomy or partial book/links.
    let mut forged = new_book(
        &format!("ci:taxonomy:forged:{stamp}"),
        "ru",
        "Forged taxonomy",
        &"s".repeat(43),
    )
    .unwrap();
    forged.taxonomy_ids.genre_ids.push(i64::MAX);
    forged.genres.push("fake".into());
    assert_eq!(
        author.create_owned_book(user, &forged).await,
        Err(AuthorError::Invalid)
    );
    let forged_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM books WHERE source_identity=$1")
            .bind(&forged.source_identity)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(forged_count, 0, "transaction must roll back book insert");

    sqlx::query("UPDATE book_taxonomy_terms SET status='blocked' WHERE id=$1")
        .bind(tag)
        .execute(&admin_pool)
        .await
        .unwrap();
    assert_eq!(
        author
            .resolve_taxonomy_selection(&selected)
            .await
            .unwrap_err(),
        AuthorError::Invalid,
    );
    assert!(author
        .suggest_taxonomies("tag", "синоним", 15)
        .await
        .unwrap()
        .is_empty());
    pool.close().await;
    admin_pool.close().await;
}
