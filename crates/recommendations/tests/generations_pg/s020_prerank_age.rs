//! S020/#486: current discovery rights precede the catalogue result limit.
//! A retained legacy generation cannot displace a visible basic new release.
use super::*;

#[tokio::test]
#[ignore = "requires disposable isolated PostgreSQL /tor_ci"]
async fn s020_age_filter_precedes_catalog_limit_despite_retained_generation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never touch production");
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = format!("ci:s020:prerank:{stamp}:");
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 prerank viewer','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-prerank-{stamp}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let seed: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,visibility_status,access_level)
         VALUES($1,'ru','S020 eligible seed','public','active','basic') RETURNING id",
    )
    .bind(format!("{prefix}seed"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let basic: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,visibility_status,access_level)
         VALUES($1,'ru','S020 eligible lower-ranked basic','public','active','basic') RETURNING id",
    )
    .bind(format!("{prefix}basic"))
    .fetch_one(&pool)
    .await
    .unwrap();

    // 510 newer, visible-but-21+-restricted books precede one basic book.
    // Apply live visibility before LIMIT even when a legacy generation exists.
    let inserted = sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,visibility_status,access_level)
         SELECT $1||'restricted:'||n::text,'ru',
                'S020 21+ candidate '||n::text,'public','active','pro_adult'
         FROM generate_series(1,510) AS n",
    )
    .bind(&prefix)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(inserted.rows_affected(), 510);
    let restricted: Vec<String> = sqlx::query_scalar(
        "SELECT id::text FROM books WHERE source_identity LIKE $1 AND access_level='pro_adult'",
    )
    .bind(format!("{prefix}%"))
    .fetch_all(&pool)
    .await
    .unwrap();

    let store = PgRecommendations::new(pool.clone());
    let old_policy = store.runtime_status().await.unwrap();
    let generation = store.request_refresh().await.unwrap();
    let candidates = sqlx::query(
        "INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         SELECT $1::uuid,$2,b.id,'content','similar_by_content',1000
         FROM books b
         WHERE b.source_identity LIKE $3 AND b.access_level='pro_adult'",
    )
    .bind(&generation)
    .bind(seed)
    .bind(format!("{prefix}%"))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(candidates.rows_affected(), 510);
    sqlx::query(
        "INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         VALUES($1::uuid,$2,$3,'content','similar_by_content',200)",
    )
    .bind(&generation)
    .bind(seed)
    .bind(basic)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE rec_generations SET status='validated',finished_at=now(),
                manifest_hash=decode(repeat('a5',32),'hex')
         WHERE id=$1::uuid AND status='building'",
    )
    .bind(&generation)
    .execute(&pool)
    .await
    .unwrap();
    store
        .activate_generation(&generation, old_policy.2, 100)
        .await
        .unwrap();

    let guest = store.feed(None, Some(seed), 20).await.unwrap();
    assert!(guest.generation_id.is_none());
    assert!(
        guest
            .items
            .iter()
            .any(|item| item.book_id == basic.to_string()),
        "inaccessible newer books must not consume the catalogue limit"
    );
    assert!(
        guest
            .items
            .iter()
            .all(|item| !restricted.contains(&item.book_id)),
        "anonymous feed must not return restricted book metadata"
    );

    let unverified = store.feed(Some(actor), Some(seed), 20).await.unwrap();
    assert!(unverified.generation_id.is_none());
    assert!(unverified
        .items
        .iter()
        .any(|item| item.book_id == basic.to_string()));
    assert!(unverified
        .items
        .iter()
        .all(|item| !restricted.contains(&item.book_id)));
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let verified = store.feed(Some(actor), Some(seed), 20).await.unwrap();
    assert!(verified.generation_id.is_none());
    assert!(verified
        .items
        .iter()
        .any(|item| restricted.contains(&item.book_id)));
    assert!(verified.items.iter().all(|item| item.source == "catalog"));
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let revoked = store.feed(Some(actor), Some(seed), 20).await.unwrap();
    assert!(revoked.generation_id.is_none());
    assert!(revoked
        .items
        .iter()
        .any(|item| item.book_id == basic.to_string()));
    assert!(revoked
        .items
        .iter()
        .all(|item| !restricted.contains(&item.book_id)));

    // Restore the exact shared CI rollout state, then remove only this test's
    // data (no production migrations or external object storage).
    sqlx::query(
        "UPDATE rec_runtime_policy
         SET active_generation=$1::uuid,previous_generation=$2::uuid,
             revision=$3,rollout_percent=$4,hold=$5 WHERE singleton",
    )
    .bind(old_policy.0)
    .bind(old_policy.1)
    .bind(old_policy.2)
    .bind(old_policy.3)
    .bind(old_policy.4)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM rec_generations WHERE id=$1::uuid")
        .bind(&generation)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE source_identity LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
