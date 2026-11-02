//! S058: real PostgreSQL lifecycle. Only disposable tor_ci.
use sqlx::PgPool;
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book,save_chapter,AuthorError,SaveChapterRequest},
};
use tor_books::repositories::postgres::DueReleaseOutcome;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn scheduled_free_release_is_owner_only_cancelable_and_exactly_once() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let pool = PgPool::connect(&url).await.expect("PostgreSQL");
    let author = PgAuthoring::new(pool.clone());
    author.probe_scheduled_releases().await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-owner@tor-ci.example','S058 Owner','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('s058-outsider@tor-ci.example','S058 Outsider','$argon2id$test')
         ON CONFLICT(email_normalized) DO UPDATE SET display_name=EXCLUDED.display_name
         RETURNING id")
        .fetch_one(&pool).await.unwrap();
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap().as_nanos();
    let book = new_book(
        &format!("tor-ci:s058:{owner}:{nonce}"), "ru",
        "Проверка отложенного выпуска", &"a".repeat(43),
    ).unwrap();
    let book_id = author.create_owned_book(owner,&book).await.unwrap();
    let first = save_chapter(SaveChapterRequest {
        book_id:book_id as u64,chapter_id:None,number:1,expected_revision:0,
        title:"Первая глава",body:"Публичное начало",access:"free",
        price_amount:0,command_key:&"b".repeat(43),
    }).unwrap();
    author.create_free_chapter(owner,&first).await.unwrap();
    author.release_book(owner,book_id).await.unwrap();
    let next = save_chapter(SaveChapterRequest {
        book_id:book_id as u64,chapter_id:None,number:2,expected_revision:0,
        title:"Вторая глава",body:"Секрет до срока",access:"free",
        price_amount:0,command_key:&"c".repeat(43),
    }).unwrap();
    let saved = author.create_pending_free_chapter(owner,&next).await.unwrap();
    let utc: String = sqlx::query_scalar(
        "SELECT to_char((now()+interval '1 hour') AT TIME ZONE 'UTC',
                'YYYY-MM-DD\"T\"HH24:MI')").fetch_one(&pool).await.unwrap();
    assert_eq!(author.schedule_pending_free_chapter(
        outsider,book_id,saved.chapter_id,1,saved.version_id,&utc).await,
        Err(AuthorError::Missing));
    assert_eq!(author.schedule_pending_free_chapter(
        owner,book_id,saved.chapter_id,2,saved.version_id,&utc).await,
        Err(AuthorError::Conflict));
    author.schedule_pending_free_chapter(
        owner,book_id,saved.chapter_id,1,saved.version_id,&utc).await.unwrap();
    let schedule = author.scheduled_free_chapter(owner,book_id,saved.chapter_id)
        .await.unwrap().unwrap();
    assert_eq!(schedule.status,"pending");
    assert!(author.scheduled_free_chapter(outsider,book_id,saved.chapter_id)
        .await.unwrap().is_none());
    let before: Option<i64> = sqlx::query_scalar(
        "SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
        .bind(saved.chapter_id).fetch_optional(&pool).await.unwrap();
    assert_eq!(before,None,"scheduling does not publish");
    assert_eq!(author.cancel_scheduled_free_chapter(
        outsider,book_id,saved.chapter_id).await,Err(AuthorError::Missing));
    author.cancel_scheduled_free_chapter(owner,book_id,saved.chapter_id)
        .await.unwrap();
    assert_eq!(author.scheduled_free_chapter(owner,book_id,saved.chapter_id)
        .await.unwrap().unwrap().status,"cancelled");
    author.schedule_pending_free_chapter(
        owner,book_id,saved.chapter_id,1,saved.version_id,&utc).await.unwrap();
    // Advance only the disposable fixture's deadline, no wall-clock waiting.
    sqlx::query(
        "UPDATE chapter_scheduled_releases
         SET scheduled_at=now()-interval '1 second' WHERE chapter_id=$1")
        .bind(saved.chapter_id).execute(&pool).await.unwrap();
    let other = author.clone();
    let (a,b) = tokio::join!(
        author.process_one_due_scheduled_release(),
        other.process_one_due_scheduled_release(),
    );
    let found = [a.unwrap(),b.unwrap()];
    assert!(found.iter().any(|v| matches!(
        v, Some((id,DueReleaseOutcome::Published)) if *id==saved.chapter_id
    )));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1")
        .bind(saved.chapter_id).fetch_one(&pool).await.unwrap();
    assert_eq!(count,1,"two workers cannot notify twice");
    let version: i64 = sqlx::query_scalar(
        "SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
        .bind(saved.chapter_id).fetch_one(&pool).await.unwrap();
    assert_eq!(version,saved.version_id);
    assert_eq!(author.scheduled_free_chapter(owner,book_id,saved.chapter_id)
        .await.unwrap().unwrap().status,"published");
    assert_eq!(author.cancel_scheduled_free_chapter(
        owner,book_id,saved.chapter_id).await,Err(AuthorError::Conflict));

    let third = save_chapter(SaveChapterRequest {
        book_id:book_id as u64,chapter_id:None,number:3,expected_revision:0,
        title:"Третья глава",body:"Старый скрытый вариант",access:"free",
        price_amount:0,command_key:&"d".repeat(43),
    }).unwrap();
    let saved3=author.create_pending_free_chapter(owner,&third).await.unwrap();
    author.schedule_pending_free_chapter(
        owner,book_id,saved3.chapter_id,1,saved3.version_id,&utc).await.unwrap();
    let update = save_chapter(SaveChapterRequest {
        book_id:book_id as u64,chapter_id:Some(saved3.chapter_id as u64),
        number:3,expected_revision:1,title:"Третья глава исправлена",
        body:"Новый текст без автопубликации",access:"free",
        price_amount:0,command_key:&"e".repeat(43),
    }).unwrap();
    let newer=author.edit_unpublished_free_chapter(owner,&update).await.unwrap();
    assert_ne!(newer.version_id,saved3.version_id);
    sqlx::query("UPDATE chapter_scheduled_releases
                 SET scheduled_at=now()-interval '1 second' WHERE chapter_id=$1")
        .bind(saved3.chapter_id).execute(&pool).await.unwrap();
    let processed=author.process_one_due_scheduled_release().await.unwrap();
    assert_eq!(processed,Some((saved3.chapter_id,DueReleaseOutcome::Stale)));
    assert_eq!(author.scheduled_free_chapter(owner,book_id,saved3.chapter_id)
        .await.unwrap().unwrap().status,"stale");
    let none: Option<i64> = sqlx::query_scalar(
        "SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
        .bind(saved3.chapter_id).fetch_optional(&pool).await.unwrap();
    assert!(none.is_none(),"edited version cannot be released by old schedule");
    pool.close().await;
}
