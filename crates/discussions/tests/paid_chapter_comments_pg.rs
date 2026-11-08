//! S011: real PostgreSQL D/A authorization for paid immutable-edition threads.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_discussions::repositories::{
    paid_chapter_comments::PgPaidChapterComments,
    public_chapter_comments::{CreateResult, NewAnchor, NewChapterComment},
};

#[tokio::test]
#[ignore = "requires isolated migrated PostgreSQL tor_ci"]
async fn s011_paid_comments_d_authorizes_exact_pinned_version_and_session() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(
        url.starts_with("postgresql://tor_app:")
            && url.ends_with("/tor_ci")
            && url.contains("@127.0.0.1:5432/")
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // E's feature boundary intentionally excludes tor-accounts. Seed only A's
    // canonical persisted facts in isolated tor_ci, never an invented role.
    let buyer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S011 buyer','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("s011-buyer-{stamp}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S011 stranger','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("s011-stranger-{stamp}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let buyer_digest = [21u8; 32];
    let other_digest = [22u8; 32];
    for (actor, digest) in [(buyer, &buyer_digest), (stranger, &other_digest)] {
        sqlx::query(
            "INSERT INTO sessions(user_id,token_hash,expires_at)
                     VALUES($1,$2,now()+interval '1 hour')",
        )
        .bind(actor)
        .bind(digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    }
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,owner_user_id,visibility,access_level)
         VALUES($1,'ru','S011 paid comments',$2,'public','basic') RETURNING id",
    ).bind(format!("ci:s011:paid:{stamp}")).bind(buyer).fetch_one(&pool).await.unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Paid chapter','public','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(chapter)
    .bind("А😀Б\n\nТолько редакция V1")
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    let purchase: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
         (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,$4,100,'RUB') RETURNING id",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(v1)
    .bind(format!("s011-paid-{stamp}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgPaidChapterComments::new(pool.clone());
    let key = "A".repeat(43);
    let hash = [13u8; 32];
    let comment = || NewChapterComment {
        chapter_id: chapter,
        version_id: v1,
        body: "Комментарий к 😀",
        command_key: &key,
        payload_hash: &hash,
        parent_id: None,
        anchor: Some(NewAnchor {
            block_index: 1,
            start_utf16: 1,
            end_utf16: 3,
            quote: "😀",
        }),
    };
    assert_eq!(
        repo.create(stranger, &other_digest, purchase, comment())
            .await,
        CreateResult::NotFound
    );
    assert!(repo
        .list(stranger, &other_digest, (purchase, chapter, v1), None, 25)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.create(buyer, &buyer_digest, 999_999, comment()).await,
        CreateResult::NotFound
    );
    let inserted = repo.create(buyer, &buyer_digest, purchase, comment()).await;
    let comment_id = match inserted {
        CreateResult::Created(id) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        repo.create(buyer, &buyer_digest, purchase, comment()).await,
        CreateResult::Replayed(comment_id)
    );
    assert_eq!(
        repo.create(
            buyer,
            &buyer_digest,
            purchase,
            NewChapterComment {
                anchor: Some(NewAnchor {
                    quote: "НЕВЕРНО",
                    ..comment().anchor.unwrap()
                }),
                command_key: "B".repeat(43).as_str(),
                ..comment()
            }
        )
        .await,
        CreateResult::Invalid
    );
    let visible = repo
        .list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(visible.items.len(), 1);
    assert_eq!(visible.items[0].body, "Комментарий к 😀");
    assert_eq!(visible.items[0].anchor.as_ref().unwrap().start_utf16, 1);

    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'PRIVATE-V2') RETURNING id",
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
    assert!(repo
        .list(buyer, &buyer_digest, (purchase, chapter, v2), None, 25)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.create(
            buyer,
            &buyer_digest,
            purchase,
            NewChapterComment {
                version_id: v2,
                ..comment()
            }
        )
        .await,
        CreateResult::NotFound
    );
    assert_eq!(
        repo.list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1,
        "purchase V1 remains pinned after V2"
    );

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1
    );
    sqlx::query("UPDATE books SET access_level='pro_adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
        .await
        .unwrap()
        .is_none());
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
        1
    );
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    assert!(repo
        .list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
        .await
        .unwrap()
        .is_none());

    // Session revocation is independent of payment/age and must immediately
    // invalidate comment access, without deleting the canonical discussion.
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(buyer_digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .list(buyer, &buyer_digest, (purchase, chapter, v1), None, 25)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.create(buyer, &buyer_digest, purchase, comment()).await,
        CreateResult::NotFound
    );
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discussion_comments WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retained, 1);
    pool.close().await;
}
