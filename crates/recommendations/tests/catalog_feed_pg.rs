//! Production catalogue serving must not consult or create an analytics profile.
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_recommendations::{
    domain::feedback::{Choice, Feed, FeedbackError},
    repositories::PgRecommendations,
    services::feedback::new_ticket,
};

struct Fixture {
    pool: PgPool,
    store: PgRecommendations,
    actors: [i64; 2],
    books: Vec<i64>,
    url: String,
}

impl Fixture {
    async fn new(name: &str) -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
        assert!(url.contains("127.0.0.1:5432") && url.ends_with("/tor_ci"));
        let pool = PgPool::connect(&url).await.unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut actors = [0; 2];
        for (index, actor) in actors.iter_mut().enumerate() {
            *actor = sqlx::query_scalar(
                "INSERT INTO users(email_normalized,display_name,password_hash)
                 VALUES($1,'Catalogue reader','$argon2id$test') RETURNING id",
            )
            .bind(format!("catalog-{name}-{stamp}-{index}@tor-ci.example"))
            .fetch_one(&pool)
            .await
            .unwrap();
        }
        let mut books = Vec::new();
        for index in 0..5 {
            let book = sqlx::query_scalar(
                "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
                 VALUES($1,'ru',$2,'public','basic') RETURNING id",
            )
            .bind(format!("ci:catalog-feed:{name}:{stamp}:{index}"))
            .bind(format!("Catalogue book {index}"))
            .fetch_one(&pool)
            .await
            .unwrap();
            books.push(book);
        }
        Self {
            store: PgRecommendations::new(pool.clone()),
            pool,
            actors,
            books,
            url,
        }
    }

    async fn cleanup(self) {
        sqlx::query("DELETE FROM user_level_grants WHERE user_id=ANY($1::bigint[])")
            .bind(self.actors.as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=ANY($1::bigint[])")
            .bind(self.actors.as_slice())
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
            .bind(&self.books)
            .execute(&self.pool)
            .await
            .unwrap();
        self.pool.close().await;
    }
}

fn ids(feed: &Feed) -> Vec<i64> {
    feed.items
        .iter()
        .map(|item| item.book_id.parse().unwrap())
        .collect()
}

fn assert_catalog(feed: &Feed) {
    assert!(feed.generation_id.is_none() && feed.next_cursor.is_none());
    assert!(!feed.personalized);
    assert!(feed.items.iter().all(|item| {
        item.source == "catalog"
            && item.reason == "new_release"
            && item.algorithm_version == "catalog_new_releases_v1"
            && item.ticket.is_none()
    }));
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn legacy_history_and_active_behavioral_generation_cannot_change_new_releases() {
    let f = Fixture::new("legacy").await;
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

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn explicit_hides_are_actor_local_and_catalogue_gets_are_read_only() {
    let f = Fixture::new("preferences").await;
    let actor = f.actors[0];
    let book = f.books[4];
    let key = new_ticket().unwrap();
    f.store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    let state: (String, String) = sqlx::query_as(
        "SELECT p.expires_at::text,c.created_at::text
         FROM rec_book_preferences p JOIN rec_user_preference_commands c
           ON c.user_id=p.user_id AND c.command_type='feedback'
         WHERE p.user_id=$1 AND p.book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let read_pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&f.url)
        .await
        .unwrap();
    let reader = PgRecommendations::new(read_pool.clone());
    let hidden = reader.feed(Some(actor), Some(f.books[0]), 3).await.unwrap();
    assert_catalog(&hidden);
    assert!(!ids(&hidden).contains(&book));
    for viewer in [None, Some(f.actors[1])] {
        let other = reader.feed(viewer, Some(f.books[0]), 3).await.unwrap();
        assert_eq!(ids(&other)[0], book);
        assert_catalog(&other);
    }
    let unchanged: (String, String) = sqlx::query_as(
        "SELECT p.expires_at::text,c.created_at::text
         FROM rec_book_preferences p JOIN rec_user_preference_commands c
           ON c.user_id=p.user_id AND c.command_type='feedback'
         WHERE p.user_id=$1 AND p.book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, state);
    let untouched: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM rec_subjects WHERE user_id=ANY($1::bigint[])),
                (SELECT count(*) FROM rec_preference_accounts WHERE user_id=$2)",
    )
    .bind(f.actors.as_slice())
    .bind(f.actors[1])
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(untouched, (0, 0));
    f.store
        .set_feedback(actor, book, Choice::Clear, 1, &new_ticket().unwrap())
        .await
        .unwrap();
    f.store
        .set_feedback(actor, book, Choice::NotInterested, 0, &key)
        .await
        .unwrap();
    let restored = reader.feed(Some(actor), Some(f.books[0]), 3).await.unwrap();
    assert_eq!(ids(&restored)[0], book);
    assert_eq!(restored.items[0].feedback_revision, Some(2));
    read_pool.close().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn catalog_checks_live_book_account_and_age_rights_before_limit() {
    let f = Fixture::new("rights").await;
    let actor = f.actors[0];
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(f.books[2])
        .execute(&f.pool)
        .await
        .unwrap();
    for (book, tier) in [(f.books[3], "adult"), (f.books[4], "pro_adult")] {
        sqlx::query("UPDATE books SET access_level=$2 WHERE id=$1")
            .bind(book)
            .bind(tier)
            .execute(&f.pool)
            .await
            .unwrap();
    }
    for viewer in [None, Some(actor)] {
        let basic = f.store.feed(viewer, Some(f.books[0]), 1).await.unwrap();
        assert_eq!(ids(&basic), vec![f.books[1]]);
        for restricted in &f.books[2..] {
            assert_eq!(
                f.store.feed(viewer, Some(*restricted), 20).await.err(),
                Some(FeedbackError::NotFound)
            );
        }
    }
    sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
        .bind(actor)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        ids(&f
            .store
            .feed(Some(actor), Some(f.books[0]), 1)
            .await
            .unwrap()),
        vec![f.books[3]]
    );
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',true)",
    )
    .bind(actor)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        ids(&f
            .store
            .feed(Some(actor), Some(f.books[0]), 1)
            .await
            .unwrap()),
        vec![f.books[4]]
    );
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f
        .store
        .feed(Some(actor), None, 20)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        f.store.feed(Some(actor), Some(f.books[0]), 20).await.err(),
        Some(FeedbackError::NotFound)
    );
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(f.books[0])
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.store.feed(None, Some(f.books[0]), 20).await.err(),
        Some(FeedbackError::NotFound)
    );
    for (viewer, seed, limit) in [
        (None, None, 0),
        (None, None, 21),
        (Some(0), None, 1),
        (None, Some(0), 1),
    ] {
        assert_eq!(
            f.store.feed(viewer, seed, limit).await.err(),
            Some(FeedbackError::Invalid)
        );
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn catalog_does_not_wait_for_legacy_history_generation_or_rollout_tables() {
    let f = Fixture::new("locked").await;
    let mut legacy = f.pool.begin().await.unwrap();
    sqlx::query(
        "LOCK TABLE rec_subjects,rec_reader_signals,rec_generations,
         rec_candidate_edges,rec_runtime_policy IN ACCESS EXCLUSIVE MODE",
    )
    .execute(&mut *legacy)
    .await
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        f.store.feed(Some(f.actors[0]), Some(f.books[0]), 3),
    )
    .await;
    legacy.rollback().await.unwrap();
    let feed = result
        .expect("catalogue GET must not depend on legacy analytics locks")
        .unwrap();
    assert_catalog(&feed);
    assert_eq!(ids(&feed), vec![f.books[4], f.books[3], f.books[2]]);
    f.cleanup().await;
}
