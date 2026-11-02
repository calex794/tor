//! C-16: D-owned read links are actor-scoped and revocation-sensitive.
use sqlx::PgPool;
use tor_commerce::repositories::entitlements::{PgDirectEntitlements, ReadablePurchaseLink};
use tor_contracts::paid_delivery::PositiveId;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn c_16_owned_links_follow_exact_authorized_purchase_and_refund() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable database");
    assert!(url.ends_with("/tor_ci"), "do not test production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut readers = Vec::new();
    for role in ["buyer", "stranger"] {
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'C16 reader','$argon2id$fixture') RETURNING id",
        )
        .bind(format!("c16-d-rights-{role}-{nonce}@example.invalid"))
        .fetch_one(&pool)
        .await
        .unwrap();
        readers.push(actor);
    }
    let buyer = readers[0];
    let outsider = readers[1];
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','C16 paid book','public') RETURNING id",
    )
    .bind(format!("ci:c16:owned:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Paid','public','paid',19900) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'First version') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    let purchase_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
           (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,$4,19900,'RUB') RETURNING id",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(v1)
    .bind(format!("c16-settlement-{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let d = PgDirectEntitlements::new(pool.clone());
    let buyer_id = PositiveId::new(buyer).unwrap();
    let outsider_id = PositiveId::new(outsider).unwrap();
    assert_eq!(
        d.readable_purchase_links(buyer_id, &[]).await.unwrap(),
        vec![]
    );
    assert_eq!(
        d.readable_purchase_links(outsider_id, &[chapter])
            .await
            .unwrap(),
        vec![]
    );
    assert_eq!(
        d.readable_purchase_links(buyer_id, &[chapter])
            .await
            .unwrap(),
        vec![ReadablePurchaseLink {
            chapter_id: chapter,
            purchase_id
        }]
    );
    // A later published edition never destroys the purchased V1 link.
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Second version') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        d.readable_purchase_links(buyer_id, &[chapter])
            .await
            .unwrap()
            .len(),
        1
    );
    // A moderated chapter must not be marked as readable.
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(d
        .readable_purchase_links(buyer_id, &[chapter])
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES($1,$2,'refund')",
    )
    .bind(purchase_id)
    .bind(format!("c16-refund-{nonce}"))
    .execute(&pool)
    .await
    .unwrap();
    assert!(d
        .readable_purchase_links(buyer_id, &[chapter])
        .await
        .unwrap()
        .is_empty());
    pool.close().await;
}
