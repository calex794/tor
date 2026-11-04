//! S020: candidate materialization is broader than anonymous indexability.
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_generation_includes_age_tiers_but_anon_and_revoked_actor_cannot_see_them() {
    let (pool, store, actor, books) = setup().await;
    let old_policy: (Option<String>, Option<String>, i64, i16, bool) = sqlx::query_as(
        "SELECT active_generation::text,previous_generation::text,revision,rollout_percent,hold
         FROM rec_runtime_policy WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET adult_enabled=FALSE,account_state='active' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut created = Vec::new();
    for (label, tier, audience, lifecycle) in [
        ("adult", Some("adult"), "adult", "in_progress"),
        ("pro", Some("pro_adult"), "general", "in_progress"),
        ("unknown", None, "general", "in_progress"),
        ("unreviewed", Some("basic"), "unreviewed", "in_progress"),
        ("pending", Some("adult"), "adult", "pending_deletion"),
    ] {
        let book: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,
                               visibility_status,lifecycle_status,access_level,genres,tags)
             VALUES($1,'ru',$2,'public','active',$3,$4,
                    ARRAY['rec13-generation'],ARRAY['space']) RETURNING id",
        )
        .bind(format!("ci:s020:rec-generation:{nonce}:{label}"))
        .bind(format!("REC tier candidate {label} {nonce}"))
        .bind(lifecycle)
        .bind(tier)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,$2)")
            .bind(book)
            .bind(audience)
            .execute(&pool)
            .await
            .unwrap();
        created.push(book);
    }
    let (adult, pro, unknown, unreviewed, pending) =
        (created[0], created[1], created[2], created[3], created[4]);

    let generation = build(&store).await;
    let content_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT book_id FROM rec_candidate_edges
         WHERE generation_id=$1::uuid AND seed_id=$2 AND source='content'",
    )
    .bind(&generation)
    .bind(books[0])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(content_ids.contains(&adult));
    assert!(content_ids.contains(&pro));
    for id in [unknown, unreviewed, pending] {
        assert!(
            !content_ids.contains(&id),
            "excluded tier must not materialize"
        );
    }
    let bad_global: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rec_candidate_edges
         WHERE generation_id=$1::uuid AND seed_id=0 AND book_id=ANY($2::bigint[])",
    )
    .bind(&generation)
    .bind(&[adult, pro])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(bad_global, 0, "anonymous global seeds remain basic-only");

    let revision = store.runtime_status().await.unwrap().2;
    store
        .activate_generation(&generation, revision, 100)
        .await
        .unwrap();
    let guest = store.feed(None, Some(books[0]), 20).await.unwrap();
    assert!(guest
        .items
        .iter()
        .all(|b| { b.book_id != adult.to_string() && b.book_id != pro.to_string() }));

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let enabled = store.feed(Some(actor), Some(books[0]), 20).await.unwrap();
    assert!(enabled.items.iter().any(|b| b.book_id == adult.to_string()));
    assert!(enabled.items.iter().all(|b| b.book_id != pro.to_string()));
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)
         ON CONFLICT(user_id,level) DO UPDATE SET is_active=TRUE",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let both = store.feed(Some(actor), Some(books[0]), 20).await.unwrap();
    assert!(both.items.iter().any(|b| b.book_id == adult.to_string()));
    assert!(both.items.iter().any(|b| b.book_id == pro.to_string()));

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let only_pro = store.feed(Some(actor), Some(books[0]), 20).await.unwrap();
    assert!(only_pro.items.iter().any(|b| b.book_id == pro.to_string()));
    assert!(only_pro
        .items
        .iter()
        .all(|b| b.book_id != adult.to_string()));
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let revoked = store.feed(Some(actor), Some(books[0]), 20).await.unwrap();
    assert!(revoked
        .items
        .iter()
        .all(|b| { b.book_id != adult.to_string() && b.book_id != pro.to_string() }));

    sqlx::query(
        "UPDATE rec_runtime_policy SET active_generation=$1::uuid,
         previous_generation=$2::uuid,revision=$3,rollout_percent=$4,hold=$5
         WHERE singleton",
    )
    .bind(old_policy.0)
    .bind(old_policy.1)
    .bind(old_policy.2)
    .bind(old_policy.3)
    .bind(old_policy.4)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
        .bind(&created)
        .execute(&pool)
        .await
        .unwrap();
}
