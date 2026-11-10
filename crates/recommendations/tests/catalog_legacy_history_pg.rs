//! Production catalogue serving with retained history on explicit baseline191.
//! Ordinary current-schema catalogue behavior remains in catalog_feed_pg.
use sqlx::PgPool;

#[path = "support/legacy_analytics.rs"]
mod legacy_analytics;
#[path = "catalog_feed_pg/fixtures.rs"]
mod fixtures;
use fixtures::*;

async fn fixture(name: &str) -> Fixture {
    let url = legacy_analytics::url("LEGACY_ANALYTICS_DATABASE_URL", "tor_app");
    let pool = PgPool::connect(&url).await.unwrap();
    legacy_analytics::assert_schema(&pool).await;
    Fixture::new(name, pool).await
}

#[tokio::test]
#[ignore = "requires explicit pre-retirement 191 PostgreSQL"]
async fn legacy_history_and_active_behavioral_generation_cannot_change_new_releases() {
    let f = fixture("legacy").await;
    let old_policy = f.store.runtime_status().await.unwrap();
    // Deliberate retained old-model state, not a new captured C observation.
    let subject: String = sqlx::query_scalar(
        "INSERT INTO rec_subjects(user_id,enabled,enabled_at)
         VALUES($1,true,now()-interval '1 day') RETURNING subject_id::text",
    )
    .bind(f.actors[0])
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rec_reader_signals(subject_id,book_id,occurred_at,source_event_id)
         VALUES($1::uuid,$2,now(),gen_random_uuid())",
    )
    .bind(&subject)
    .bind(f.books[4])
    .execute(&f.pool)
    .await
    .unwrap();
    let generation: String =
        sqlx::query_scalar("INSERT INTO rec_generations DEFAULT VALUES RETURNING id::text")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    for (seed, book, source, reason) in [
        (f.books[0], f.books[2], "behavioral", "readers_also_read"),
        (f.books[4], f.books[1], "behavioral", "readers_also_read"),
        (0, f.books[1], "popularity", "popular"),
        (f.books[0], f.books[1], "related_books", "manually_related"),
    ] {
        sqlx::query(
            "INSERT INTO rec_candidate_edges(generation_id,seed_id,book_id,source,reason,score)
             VALUES($1::uuid,$2,$3,$4,$5,1000)",
        )
        .bind(&generation)
        .bind(seed)
        .bind(book)
        .bind(source)
        .bind(reason)
        .execute(&f.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE rec_generations SET status='validated',finished_at=now(),
         manifest_hash=decode(repeat('a5',32),'hex') WHERE id=$1::uuid",
    )
    .bind(&generation)
    .execute(&f.pool)
    .await
    .unwrap();
    let revision = f
        .store
        .activate_generation(&generation, old_policy.2, 100)
        .await
        .unwrap();
    let expected = vec![f.books[4], f.books[3], f.books[2]];
    for actor in [None, Some(f.actors[0]), Some(f.actors[1])] {
        let feed = f.store.feed(actor, Some(f.books[0]), 3).await.unwrap();
        assert_catalog(&feed);
        assert_eq!(ids(&feed), expected);
        assert!(feed
            .items
            .iter()
            .all(|item| { item.feedback_revision == actor.map(|_| 0) }));
    }
    // Legacy hold and rollout changes also have no effect on this model.
    f.store.hold(revision).await.unwrap();
    let held = f
        .store
        .feed(Some(f.actors[0]), Some(f.books[0]), 3)
        .await
        .unwrap();
    assert_eq!(ids(&held), expected);
    assert_catalog(&held);
    sqlx::query(
        "UPDATE rec_runtime_policy SET active_generation=$1::uuid,previous_generation=$2::uuid,
         revision=$3,rollout_percent=$4,hold=$5 WHERE singleton",
    )
    .bind(old_policy.0)
    .bind(old_policy.1)
    .bind(old_policy.2)
    .bind(old_policy.3)
    .bind(old_policy.4)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM rec_generations WHERE id=$1::uuid")
        .bind(&generation)
        .execute(&f.pool)
        .await
        .unwrap();
    f.cleanup().await;
}
