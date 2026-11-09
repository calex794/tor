//! S016/S017 disposable PostgreSQL acceptance. No production credentials.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_social::repositories::book_shares::{BookShareError, PgBookShares};

async fn fixture() -> (PgPool, i64, i64, i64, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL only");
    assert!(url.contains("127.0.0.1:5432") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for n in 0..3 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,$2,'$argon2id$fixture') RETURNING id",
        )
        .bind(format!("share-{stamp}-{n}@example.invalid"))
        .bind(format!("ShareUser{stamp}{n}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(id);
    }
    let mut books = Vec::new();
    for (i, visibility, level) in [
        (0, "public", "basic"),
        (1, "public", "adult"),
        (2, "draft", "basic"),
    ] {
        let book:i64=sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
             VALUES($1,'ru',$2,$3,$4,$5) RETURNING id"
        )
        .bind(format!("ci:share:{stamp}:{i}"))
        .bind(format!("BookShareTest{stamp}-{i}"))
        .bind(visibility).bind(level).bind(users[0])
        .fetch_one(&pool).await.unwrap();
        books.push(book);
    }
    (
        pool, users[0], users[1], users[2], books[0], books[1], books[2],
    )
}
async fn friends(pool: &PgPool, a: i64, b: i64) {
    sqlx::query(
        "INSERT INTO social_friendships(user_low,user_high,requester_id,status,action_user_id,accepted_at)
         VALUES($1,$2,$3,'accepted',$4,clock_timestamp())"
    )
    .bind(a.min(b)).bind(a.max(b)).bind(a).bind(b)
    .execute(pool).await.unwrap();
}
fn key(n: u64) -> String {
    format!("{n:043}")
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s016_share_requires_friend_public_book_and_no_entitlements() {
    let (pool, sender, receiver, outsider, book, adult, draft) = fixture().await;
    let shares = PgBookShares::new(pool.clone());
    assert_eq!(
        shares.send(sender, receiver, book, "Hi", &key(1)).await,
        Err(BookShareError::Forbidden)
    );
    friends(&pool, sender, receiver).await;
    let id = shares
        .send(sender, receiver, book, "Hi", &key(1))
        .await
        .unwrap();
    assert_eq!(
        shares
            .send(sender, receiver, book, "Hi", &key(1))
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        shares.send(sender, receiver, book, "Other", &key(1)).await,
        Err(BookShareError::Conflict)
    );
    assert_eq!(
        shares.send(sender, outsider, book, "Hi", &key(2)).await,
        Err(BookShareError::Forbidden)
    );
    assert_eq!(
        shares.send(sender, receiver, adult, "", &key(3)).await,
        Err(BookShareError::NotFound)
    );
    assert_eq!(
        shares.send(sender, receiver, draft, "", &key(4)).await,
        Err(BookShareError::NotFound)
    );
    assert_eq!(shares.inbox(receiver).await.unwrap().len(), 1);
    assert!(shares.inbox(sender).await.unwrap().is_empty());
    assert!(shares.inbox(outsider).await.unwrap().is_empty());
    assert_eq!(
        shares.receiver_action(outsider, id, "hide", "", "").await,
        Err(BookShareError::NotFound)
    );
    let purchased: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM library_purchased_chapters WHERE user_id=$1",
    )
    .bind(receiver)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchased, 0);
    shares
        .receiver_action(receiver, id, "hide", "", "")
        .await
        .unwrap();
    assert!(shares.inbox(receiver).await.unwrap().is_empty());
    assert_eq!(
        shares.receiver_action(receiver, id, "hide", "", "").await,
        Ok(())
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s017_report_and_mute_are_separate_recipient_owned_actions() {
    let (pool, sender, receiver, _, book, _, _) = fixture().await;
    friends(&pool, sender, receiver).await;
    let shares = PgBookShares::new(pool.clone());
    let report = shares
        .send(sender, receiver, book, "Report", &key(5))
        .await
        .unwrap();
    shares
        .receiver_action(receiver, report, "report", "spam", "Repeated")
        .await
        .unwrap();
    assert_eq!(
        shares
            .receiver_action(receiver, report, "mute", "", "")
            .await,
        Err(BookShareError::Conflict)
    );
    let reasons: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM social_book_share_reports
         WHERE share_id=$1 AND reporter_user_id=$2 AND reason='spam' AND state='pending'",
    )
    .bind(report)
    .bind(receiver)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reasons, 1);

    let second = shares
        .send(sender, receiver, book, "Mute", &key(6))
        .await
        .unwrap();
    shares
        .receiver_action(receiver, second, "mute", "", "")
        .await
        .unwrap();
    assert!(shares.inbox(receiver).await.unwrap().is_empty());
    assert_eq!(
        shares
            .send(sender, receiver, book, "No more", &key(7))
            .await,
        Err(BookShareError::Forbidden)
    );
    let muted: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM social_book_share_mutes
         WHERE recipient_user_id=$1 AND sender_user_id=$2)",
    )
    .bind(receiver)
    .bind(sender)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(muted);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s016_block_and_parallel_replay_fail_closed() {
    let (pool, sender, receiver, _, book, _, _) = fixture().await;
    friends(&pool, sender, receiver).await;
    let service = PgBookShares::new(pool.clone());
    let token = key(11);
    let (a, b) = tokio::join!(
        service.send(sender, receiver, book, "Once", &token),
        service.send(sender, receiver, book, "Once", &token)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(service.inbox(receiver).await.unwrap().len(), 1);
    sqlx::query("INSERT INTO social_user_blocks(blocker_id,blocked_id) VALUES($1,$2)")
        .bind(receiver)
        .bind(sender)
        .execute(&pool)
        .await
        .unwrap();
    assert!(service.inbox(receiver).await.unwrap().is_empty());
    assert_eq!(
        service
            .send(sender, receiver, book, "Blocked", &key(12))
            .await,
        Err(BookShareError::Forbidden)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s016_parallel_circle_sends_lock_users_in_one_order() {
    let (pool, a, b, c, book, _, _) = fixture().await;
    friends(&pool, a, b).await;
    friends(&pool, b, c).await;
    friends(&pool, c, a).await;
    let shares = PgBookShares::new(pool);
    let keys = [key(71), key(72), key(73)];
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            shares.send(a, b, book, "AB", &keys[0]),
            shares.send(b, c, book, "BC", &keys[1]),
            shares.send(c, a, book, "CA", &keys[2])
        )
    })
    .await
    .expect("three-way delivery must not deadlock");
    assert!(result.0.is_ok(), "A->B: {:?}", result.0);
    assert!(result.1.is_ok(), "B->C: {:?}", result.1);
    assert!(result.2.is_ok(), "C->A: {:?}", result.2);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s016_sender_quota_and_moderation_visibility_hold() {
    let (pool, sender, receiver, _, book, _, _) = fixture().await;
    friends(&pool, sender, receiver).await;
    let shares = PgBookShares::new(pool.clone());
    sqlx::query("UPDATE books SET lifecycle_status='completed' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    for n in 100..110 {
        assert!(
            shares
                .send(sender, receiver, book, "", &key(n))
                .await
                .is_ok(),
            "first ten sends should be accepted"
        );
    }
    assert_eq!(
        shares.send(sender, receiver, book, "", &key(110)).await,
        Err(BookShareError::RateLimited)
    );
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert!(shares.inbox(receiver).await.unwrap().is_empty());
    assert_eq!(
        shares.send(sender, receiver, book, "", &key(111)).await,
        Err(BookShareError::NotFound)
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s016_adult_and_pro_adult_shares_follow_live_recipient_access() {
    let (pool, sender, receiver, _, basic, adult, _) = fixture().await;
    friends(&pool, sender, receiver).await;
    let pro: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
         VALUES($1,'ru','ProAdultShare','public','pro_adult',$2) RETURNING id",
    )
    .bind(format!("ci:s016:pro:{sender}:{receiver}"))
    .bind(sender)
    .fetch_one(&pool)
    .await
    .unwrap();
    let shares = PgBookShares::new(pool.clone());
    assert_eq!(
        shares.send(sender, receiver, adult, "", &key(300)).await,
        Err(BookShareError::NotFound)
    );
    assert_eq!(
        shares.send(sender, receiver, pro, "", &key(301)).await,
        Err(BookShareError::NotFound)
    );
    // Both book metadata and the sending form must fail closed for the sender.
    assert_eq!(
        shares.public_book_title(adult, sender).await,
        Err(BookShareError::NotFound)
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id IN ($1,$2)")
        .bind(sender)
        .bind(receiver)
        .execute(&pool)
        .await
        .unwrap();
    assert!(shares.public_book_title(adult, sender).await.is_ok());
    let adult_id = shares
        .send(sender, receiver, adult, "Adult", &key(302))
        .await
        .unwrap();
    assert!(shares
        .inbox(receiver)
        .await
        .unwrap()
        .iter()
        .any(|s| s.id == adult_id));
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(receiver)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!shares
        .inbox(receiver)
        .await
        .unwrap()
        .iter()
        .any(|s| s.id == adult_id));
    assert_eq!(
        shares.send(sender, receiver, adult, "", &key(303)).await,
        Err(BookShareError::NotFound)
    );
    // A pro_adult grant alone does not imply the adult opt-in required by
    // the current public reader; but it does authorize a 21+ Book.
    for user_id in [sender, receiver] {
        sqlx::query(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES($1,'pro_adult','moderation',TRUE)",
        )
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert_eq!(
        shares.send(sender, receiver, adult, "", &key(304)).await,
        Err(BookShareError::NotFound)
    );
    let pro_id = shares
        .send(sender, receiver, pro, "Pro", &key(305))
        .await
        .unwrap();
    assert!(shares
        .inbox(receiver)
        .await
        .unwrap()
        .iter()
        .any(|s| s.id == pro_id));
    sqlx::query(
        "UPDATE user_level_grants SET is_active=FALSE
         WHERE user_id=$1 AND level='pro_adult'",
    )
    .bind(receiver)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!shares
        .inbox(receiver)
        .await
        .unwrap()
        .iter()
        .any(|s| s.id == pro_id));
    assert_eq!(
        shares.send(sender, receiver, pro, "", &key(306)).await,
        Err(BookShareError::NotFound)
    );
    // Basic shares remain unaffected by age downgrades.
    let basic_id = shares
        .send(sender, receiver, basic, "", &key(307))
        .await
        .unwrap();
    assert!(shares
        .inbox(receiver)
        .await
        .unwrap()
        .iter()
        .any(|s| s.id == basic_id));
    // Delivery is a link, never an entitlement.
    let purchased: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM library_purchased_chapters WHERE user_id=$1",
    )
    .bind(receiver)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchased, 0);
}
