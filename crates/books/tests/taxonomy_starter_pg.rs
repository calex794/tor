//! Integration gate for the *review proposal* in disposable tor_ci.
//! The review CSV is applied first by tools/tests/taxonomy_csv_import_pg.py.
//! Never run against permanent databases or treat this as editorial approval.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, TaxonomySelection},
};

#[tokio::test]
#[ignore = "requires disposable tor_ci with the starter CSV imported by CI"]
async fn starter_csv_ids_are_selectable_and_persist_as_book_links() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated app role");
    assert!(url.ends_with("/tor_ci"), "refuse non-tor_ci databases");
    let pool = PgPool::connect(&url).await.unwrap();
    let author = PgAuthoring::new(pool.clone());

    // Must be the IDs created by the actual CSV importer, not a hand-built
    // fixture bypassing the curator's approval/alias contracts.
    let genre: i64 = sqlx::query_scalar(
        "SELECT id FROM book_taxonomy_terms
         WHERE kind='genre' AND label='Фэнтези' AND status='active'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let tag: i64 = sqlx::query_scalar(
        "SELECT id FROM book_taxonomy_terms
         WHERE kind='tag' AND label='Система' AND status='active'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let by_genre = author
        .suggest_taxonomies("genre", "фэнт", 15)
        .await
        .unwrap();
    assert!(by_genre.iter().any(|option| option.id == genre));

    // NFKC turns fullwidth Latin characters into the reviewed English alias.
    let by_alias = author
        .suggest_taxonomies("tag", "Ｓｙｓｔｅｍ", 15)
        .await
        .unwrap();
    assert!(by_alias.iter().any(|option| option.id == tag));

    // The same numeric canonical IDs accepted by the author form.
    let selection =
        TaxonomySelection::parse(Some(&genre.to_string()), Some(&tag.to_string()), None).unwrap();
    let names = author.resolve_taxonomy_selection(&selection).await.unwrap();
    assert_eq!(names.genres, vec!["фэнтези".to_owned()]);
    assert_eq!(names.tags, vec!["система".to_owned()]);

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Starter Taxonomy Owner','$argon2id$test') RETURNING id",
    )
    .bind(format!("taxonomy-starter-{stamp}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut book = new_book(
        &format!("ci:taxonomy:starter:{stamp}"),
        "ru",
        "Starter taxonomy selection",
        &"t".repeat(43),
    )
    .unwrap();
    book.genres = names.genres;
    book.tags = names.tags;
    book.fandoms = names.fandoms;
    book.taxonomy_ids = selection;
    let book_id = author.create_owned_book(user, &book).await.unwrap();

    let links: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind,term_id FROM book_taxonomy_links
         WHERE book_id=$1 ORDER BY kind,position",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(links, vec![("genre".to_owned(), genre), ("tag".to_owned(), tag)]);
    let persisted: (Vec<String>, Vec<String>) =
        sqlx::query_as("SELECT genres,tags FROM books WHERE id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted.0, vec!["фэнтези".to_owned()]);
    assert_eq!(persisted.1, vec!["система".to_owned()]);
}
