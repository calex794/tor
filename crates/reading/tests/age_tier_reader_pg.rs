//! S020/C: the public SQL reader must not disclose restricted Book metadata
//! or free chapter bodies without a live account session and verified tier.
use sqlx::PgPool;
use tor_reading::{repositories::PgReader, services::PublicReadingRepository};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending /tor_ci"]
async fn age_gate_pg_visitor_adult_grant_revocation_and_legacy_unknown() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Reader age gate','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-age-reader-{now}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut digest = [0u8; 32];
    digest[..16].copy_from_slice(&now.to_be_bytes());
    digest[16..].copy_from_slice(&now.to_be_bytes());
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    async fn new_book(
        pool: &PgPool,
        stamp: u128,
        name: &str,
        tier: Option<&str>,
    ) -> (u64, u64, u64) {
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES($1,'ru',$2,'public',$3) RETURNING id",
        )
        .bind(format!("ci:age:{name}:{stamp}"))
        .bind(format!("Age {name}"))
        .bind(tier)
        .fetch_one(pool)
        .await
        .unwrap();
        let free: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,1,'Free body','public','free',0) RETURNING id",
        )
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap();
        let paid: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,2,'Paid title','public','paid',100) RETURNING id",
        )
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap();
        for (chapter, body) in [(free, "S020 FREE BODY"), (paid, "S020 SECRET PAID BODY")] {
            let ver: i64 = sqlx::query_scalar(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
                 VALUES($1,1,$2) RETURNING id",
            )
            .bind(chapter)
            .bind(body)
            .fetch_one(pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
                .bind(chapter)
                .bind(ver)
                .execute(pool)
                .await
                .unwrap();
        }
        (book_id as u64, free as u64, paid as u64)
    }
    let reader = PgReader::new(pool.clone());
    let (basic, basic_free, _) = new_book(&pool, now, "basic", Some("basic")).await;
    let (adult, adult_free, adult_paid) = new_book(&pool, now, "adult", Some("adult")).await;
    let (pro, pro_free, pro_paid) = new_book(&pool, now, "pro", Some("pro_adult")).await;
    let (unknown, unknown_free, _) = new_book(&pool, now, "legacy", None).await;

    assert!(reader.public_book(basic).await.unwrap().is_some());
    assert!(reader.public_chapter(basic_free).await.unwrap().is_some());
    for (book, free, paid) in [(adult, adult_free, adult_paid), (pro, pro_free, pro_paid)] {
        assert!(reader.public_book(book).await.unwrap().is_none());
        assert!(reader.public_chapter(free).await.unwrap().is_none());
        assert!(reader
            .public_paid_chapter_preview(paid)
            .await
            .unwrap()
            .is_none());
        assert!(reader
            .public_book_page_for_session(book, 100, None, None)
            .await
            .unwrap()
            .is_none());
        assert!(reader
            .public_book_page_for_session(book, 100, None, Some(&[7u8; 32]))
            .await
            .unwrap()
            .is_none());
        assert!(reader
            .public_chapter_for_session(free, Some(&[7u8; 32]))
            .await
            .unwrap()
            .is_none());
    }
    assert!(reader.public_book(unknown).await.unwrap().is_none());
    assert!(reader.public_chapter(unknown_free).await.unwrap().is_none());
    // An authenticated but basic account cannot use a session cookie to
    // change either restricted Book's server-side effective reader tier.
    assert!(reader
        .public_chapter_for_session(adult_free, Some(&digest))
        .await
        .unwrap()
        .is_none());
    assert!(reader
        .public_chapter_for_session(pro_free, Some(&digest))
        .await
        .unwrap()
        .is_none());

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let adult_page = reader
        .public_book_page_for_session(adult, 100, None, Some(&digest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(adult_page.chapters.len(), 2);
    assert_eq!(
        reader
            .public_chapter_for_session(adult_free, Some(&digest))
            .await
            .unwrap()
            .unwrap()
            .body_markdown,
        "S020 FREE BODY"
    );
    assert!(reader
        .public_paid_chapter_preview_for_session(adult_paid, Some(&digest))
        .await
        .unwrap()
        .is_some());
    assert!(reader
        .public_book_page_for_session(pro, 100, None, Some(&digest))
        .await
        .unwrap()
        .is_none());

    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        reader
            .public_chapter_for_session(pro_free, Some(&digest))
            .await
            .unwrap()
            .unwrap()
            .body_markdown,
        "S020 FREE BODY"
    );
    assert!(reader
        .public_paid_chapter_preview_for_session(pro_paid, Some(&digest))
        .await
        .unwrap()
        .is_some());
    // Revoking a verified grant removes the 21+ capability on the next read.
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(reader
        .public_chapter_for_session(pro_free, Some(&digest))
        .await
        .unwrap()
        .is_none());
    assert!(reader
        .public_book_page_for_session(pro, 100, None, Some(&digest))
        .await
        .unwrap()
        .is_none());
    // Same response as a nonexistent Book, not a leaky restricted header.
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(adult as i64)
        .execute(&pool)
        .await
        .unwrap();
    assert!(reader
        .public_chapter_for_session(adult_free, Some(&digest))
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE books SET admin_hold=FALSE WHERE id=$1")
        .bind(adult as i64)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert!(reader
        .public_chapter_for_session(adult_free, Some(&digest))
        .await
        .unwrap()
        .is_none());
    assert!(reader
        .public_book_page_for_session(adult, 100, None, Some(&digest))
        .await
        .unwrap()
        .is_none());
    pool.close().await;
}
