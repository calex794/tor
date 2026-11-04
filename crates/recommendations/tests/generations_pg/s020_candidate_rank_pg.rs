//! S020 P0: filter actor-age eligibility before bounded hybrid ranking.
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_revoked_age_edges_cannot_crowd_out_anonymous_basic_candidates() {
    let (pool, store, _actor, books) = setup().await;
    let policy: (Option<String>, Option<String>, i64, i16, bool) = sqlx::query_as(
        "SELECT active_generation::text,previous_generation::text,revision,
                rollout_percent,hold FROM rec_runtime_policy WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Create more forbidden content edges than the hybrid top-50 threshold.
    // Their Book metadata is public, but their age tier requires live opt-in.
    let adult_ids: Vec<i64> = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,
                           visibility_status,lifecycle_status,access_level,genres,tags)
         SELECT $1::text || ':' || n::text,'ru',
                'Restricted ranking fixture ' || n::text,
                'public','active','in_progress','adult',
                ARRAY['ranking-age-fixture'],ARRAY[]::text[]
         FROM generate_series(1,80) n RETURNING id",
    )
    .bind(format!("ci:s020:rank:{nonce}"))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(adult_ids.len(), 80);

    // Old timestamp ensures it cannot be rescued by the global new-release
    // fallback. It must survive via the actual seed-specific content edge.
    let basic: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,
                           visibility_status,lifecycle_status,access_level,created_at,genres,tags)
         VALUES($1,'ru','Allowed ranking fixture','public','active',
                'in_progress','basic','2000-01-01'::timestamptz,
                ARRAY['rank-target-only'],ARRAY[]::text[]) RETURNING id",
    )
    .bind(format!("ci:s020:rank:{nonce}:basic"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let generation = store.request_refresh().await.unwrap();
    sqlx::query(
        "INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         SELECT $1::uuid,$2,id,'content','similar_by_content',1000
         FROM books WHERE id=ANY($3::bigint[])",
    )
    .bind(&generation)
    .bind(books[0])
    .bind(&adult_ids)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
         VALUES($1::uuid,$2,$3,'content','similar_by_content',950)",
    )
    .bind(&generation)
    .bind(books[0])
    .bind(basic)
    .execute(&pool)
    .await
    .unwrap();

    for _ in 0..200 {
        if !store.advance_generation().await.unwrap() {
            break;
        }
    }
    store
        .activate_generation(
            &generation,
            store.runtime_status().await.unwrap().2,
            100,
        )
        .await
        .unwrap();
    let guest = store.feed(None, Some(books[0]), 20).await.unwrap();
    assert_eq!(
        guest.generation_id.as_deref(),
        Some(generation.as_str()),
        "must use real ranked generation, not a safe fallback"
    );
    let allowed = guest
        .items
        .iter()
        .find(|item| item.book_id == basic.to_string())
        .expect("authorized basic content edge must survive pre-rank filtering");
    assert_eq!(allowed.source, "content");
    assert!(
        guest.items.iter().all(|item| {
            !adult_ids.contains(&item.book_id.parse::<i64>().unwrap())
        }),
        "guest cannot see age-tier metadata"
    );

    // Restore the exact entry policy before removing isolated fixtures.
    sqlx::query(
        "UPDATE rec_runtime_policy
         SET active_generation=$1::uuid,previous_generation=$2::uuid,
             revision=$3,rollout_percent=$4,hold=$5 WHERE singleton",
    )
    .bind(policy.0)
    .bind(policy.1)
    .bind(policy.2)
    .bind(policy.3)
    .bind(policy.4)
    .execute(&pool)
    .await
    .unwrap();
    let mut ids = adult_ids;
    ids.push(basic);
    sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
        .bind(&ids)
        .execute(&pool)
        .await
        .unwrap();
}
