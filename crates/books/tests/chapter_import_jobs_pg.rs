//! S023: disposable PostgreSQL 18 regression for durable queue claims and cancellation.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError},
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn s023_claim_fencing_cancel_and_private_status() {
    let url=std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"),"no production database");
    let pool=PgPool::connect(&url).await.unwrap();
    let repo=PgAuthoring::new(pool.clone());
    let nonce=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let actor:i64=sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Importer','$argon2id$test') RETURNING id"
    ).bind(format!("s023-job-{nonce}@tor-ci.example"))
    .fetch_one(&pool).await.unwrap();
    let stranger:i64=sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Stranger','$argon2id$test') RETURNING id"
    ).bind(format!("s023-stranger-{nonce}@tor-ci.example"))
    .fetch_one(&pool).await.unwrap();
    let book=new_book(&format!("tor-ci:s023-jobs:{nonce}"),"ru","Очередь",&"b".repeat(43)).unwrap();
    let book_id=repo.create_owned_book(actor,&book).await.unwrap();
    let key="c".repeat(43);
    let hash=[3u8;32];
    let fingerprint=[4u8;32];
    let object=format!("staging/chapters/s023-{nonce}.txt");
    let job=repo.enqueue_verified_chapter_import(
        actor,book_id,&key,&object,"source.txt",&hash,100,&fingerprint,2
    ).await.unwrap();
    assert_eq!(job,repo.enqueue_verified_chapter_import(
        actor,book_id,&key,&object,"source.txt",&hash,100,&fingerprint,2
    ).await.unwrap(),"same request is idempotent");
    assert_eq!(repo.enqueue_verified_chapter_import(
        actor,book_id,&key,&object,"source.txt",&[9u8;32],100,&fingerprint,2
    ).await,Err(AuthorError::Conflict));
    assert_eq!(repo.enqueue_verified_chapter_import(
        stranger,book_id,&"d".repeat(43),&format!("staging/chapters/other-{nonce}.txt"),
        "source.txt",&hash,100,&fingerprint,2
    ).await,Err(AuthorError::Missing));
    assert!(repo.owned_chapter_import_status(stranger,book_id,job).await.unwrap().is_none());
    assert_eq!(repo.owned_chapter_import_status(actor,book_id,job).await.unwrap().unwrap().status,"queued");
    let (a,b)=tokio::join!(repo.claim_chapter_import(),repo.claim_chapter_import());
    let claims=[a.unwrap(),b.unwrap()];
    assert_eq!(claims.iter().filter(|x|x.is_some()).count(),1,"SKIP LOCKED excludes concurrent worker");
    let claim=claims.into_iter().flatten().next().unwrap();
    assert_eq!(claim.job_id,job);
    assert!(!repo.cancel_chapter_import(stranger,book_id,job).await.unwrap());
    assert!(repo.cancel_chapter_import(actor,book_id,job).await.unwrap());
    assert!(!repo.fail_chapter_import(job,claim.epoch,false,"bad_source").await.unwrap(),
            "cancel invalidates old worker epoch");
    assert_eq!(repo.owned_chapter_import_status(actor,book_id,job).await.unwrap().unwrap().status,"cancelled");
    let saved:i64=sqlx::query_scalar("SELECT count(*) FROM chapters WHERE book_id=$1")
        .bind(book_id).fetch_one(&pool).await.unwrap();
    assert_eq!(saved,0);
    pool.close().await;
}
