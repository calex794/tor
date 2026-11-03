//! S058 NEwBOO schedule settings: disposable PG only; no worker activation.
use sqlx::PgPool;
use tor_books::{
    repositories::{book_members::PgBookMembers, postgres::PgAuthoring},
    services::authoring::{new_book, AuthorError},
};
use tor_contracts::access::BookRole;

fn times(items: &[&str]) -> Vec<String> {
    items.iter().map(|x| (*x).to_owned()).collect()
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s058_book_schedule_owner_collaborator_cadence_acl_and_revision() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never run on production");
    let pool = PgPool::connect(&url).await.expect("disposable postgres");
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-cadence-owner@tor-ci.example','Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-cadence-translator@tor-ci.example','Translator','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-cadence-stranger@tor-ci.example','Stranger','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let author = PgAuthoring::new(pool.clone());
    // Book registration claims titles globally in /tor_ci. A stable test
    // title collides with previous scenarios even when source IDs differ.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let source = format!("tor-ci:s058-book-cadence:{owner}:{nonce}");
    let title = format!("Расписание книги {nonce}");
    let key = format!("{nonce:043x}");
    let book = new_book(&source, "ru", &title, &key).unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let empty = author
        .get_book_delayed_schedule(owner, book_id)
        .await
        .unwrap();
    assert_eq!(empty.schedule_revision, 0);
    assert!(empty.publication_times.is_empty());
    assert_eq!(
        author.get_book_delayed_schedule(stranger, book_id).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author
            .replace_book_delayed_schedule(stranger, book_id, true, 2, &times(&["09:00"]))
            .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author
            .replace_book_delayed_schedule(owner, book_id, true, 0, &times(&["09:00"]))
            .await,
        Err(AuthorError::Invalid)
    );
    let slots = times(&["09:00", "09:00", "18:00"]);
    let first = author
        .replace_book_delayed_schedule(owner, book_id, true, 2, &slots)
        .await
        .unwrap();
    assert_eq!(first.schedule_revision, 1);
    assert_eq!(first.publication_times, slots);
    assert!(first.next_slot_utc.as_deref().unwrap().ends_with('Z'));
    assert!(first.anchor_date.is_some());
    assert!(first.next_slot_index.is_some());
    assert_eq!(
        author
            .replace_book_delayed_schedule(owner, book_id, true, 2, &slots)
            .await
            .unwrap()
            .schedule_revision,
        1
    );

    PgBookMembers::new(pool.clone())
        .add_collaborator(owner, book_id, translator, BookRole::Translator)
        .await
        .unwrap();
    assert_eq!(
        author
            .get_book_delayed_schedule(translator, book_id)
            .await
            .unwrap(),
        first
    );
    let changed = author
        .replace_book_delayed_schedule(translator, book_id, true, 2, &times(&["10:00"]))
        .await
        .unwrap();
    assert_eq!(changed.schedule_revision, 2);
    assert_eq!(changed.anchor_date, first.anchor_date);
    assert_eq!(changed.publication_times, times(&["10:00"]));
    let disabled = author
        .replace_book_delayed_schedule(owner, book_id, false, 2, &times(&["10:00"]))
        .await
        .unwrap();
    assert_eq!(disabled.schedule_revision, 3);
    assert!(disabled.anchor_date.is_none());
    assert!(disabled.next_slot_utc.is_none());
    assert!(disabled.next_slot_index.is_none());
    sqlx::query("UPDATE book_members SET is_active=FALSE WHERE book_id=$1 AND user_id=$2")
        .bind(book_id)
        .bind(translator)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        author.get_book_delayed_schedule(translator, book_id).await,
        Err(AuthorError::Missing)
    );
    assert_eq!(
        author
            .replace_book_delayed_schedule(translator, book_id, true, 1, &times(&["12:00"]))
            .await,
        Err(AuthorError::Missing)
    );
    let kept = author
        .get_book_delayed_schedule(owner, book_id)
        .await
        .unwrap();
    assert_eq!(kept, disabled);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM book_delayed_publication_slots WHERE book_id=$1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    pool.close().await;
}
