use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_generation_includes_classified_tiers_but_never_reveals_restricted_books_to_guests() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never touch production");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let genre = format!("s020-gen-tier-{nonce}");
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 generation reader','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-rec-generation-{nonce}@tor-ci.example"))
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut books = Vec::new();
    for (kind, level, visibility, lifecycle) in [
        ("basic", Some("basic"), "active", "in_progress"),
        ("adult", Some("adult"), "active", "in_progress"),
        ("pro", Some("pro_adult"), "active", "in_progress"),
        ("pending", Some("adult"), "active", "pending_deletion"),
        ("hidden", Some("pro_adult"), "hidden", "in_progress"),
        ("legacy", None, "active", "in_progress"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,
                               visibility_status,lifecycle_status,access_level,genres,tags)
             VALUES($1,'ru',$2,'public',$3,$4,$5,ARRAY[$6::text],ARRAY['s020-rec-tier'])
             RETURNING id",
        )
        .bind(format!("ci:s020:rec-generation:{nonce}:{kind}"))
        .bind(format!("S020 recommendation {nonce} {kind}"))
        .bind(visibility)
        .bind(lifecycle)
        .bind(level)
        .bind(&genre)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO book_recommendation_audiences(book_id,audience) VALUES($1,'general')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        books.push((kind, id));
    }
    let id = |kind: &str| books.iter().find(|(name, _)| *name == kind).unwrap().1;
    let source_ids: Vec<i64> = books.iter().map(|(_, id)| *id).collect();
    let anonymous: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM rec_public_books WHERE id=ANY($1::bigint[]) ORDER BY id",
    )
    .bind(&source_ids)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(anonymous, [id("basic")]);
    let materializable: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM rec_materializable_books WHERE id=ANY($1::bigint[]) ORDER BY id",
    )
    .bind(&source_ids)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(materializable, [id("basic"), id("adult"), id("pro")]);

    let policy: (Option<String>, Option<String>, i64, i16, bool) = sqlx::query_as(
        "SELECT active_generation::text,previous_generation::text,revision,rollout_percent,hold
         FROM rec_runtime_policy WHERE singleton",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let store = PgRecommendations::new(pool.clone());
    let generation = build(&store).await;
    let visible_restricted = vec![id("adult"), id("pro")];
    let candidates: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_candidate_edges
         WHERE generation_id=$1::uuid AND seed_id=$2
           AND book_id=ANY($3::bigint[]) AND source='content'",
    )
    .bind(&generation)
    .bind(id("basic"))
    .bind(&visible_restricted)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(candidates, 2, "source generation must cover both age tiers");
    store
        .activate_generation(&generation, store.runtime_status().await.unwrap().2, 100)
        .await
        .unwrap();

    let guest = store.feed(None, Some(id("basic")), 20).await.unwrap();
    assert!(guest
        .items
        .iter()
        .all(|item| !visible_restricted.contains(&item.book_id.parse::<i64>().unwrap())));
    let unverified = store
        .feed(Some(actor), Some(id("basic")), 20)
        .await
        .unwrap();
    assert!(unverified
        .items
        .iter()
        .all(|item| !visible_restricted.contains(&item.book_id.parse::<i64>().unwrap())));

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let adult = store
        .feed(Some(actor), Some(id("basic")), 20)
        .await
        .unwrap();
    assert!(adult
        .items
        .iter()
        .any(|item| item.book_id == id("adult").to_string()));
    assert!(adult
        .items
        .iter()
        .all(|item| item.book_id != id("pro").to_string()));

    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let both = store
        .feed(Some(actor), Some(id("basic")), 20)
        .await
        .unwrap();
    assert!(both
        .items
        .iter()
        .any(|item| item.book_id == id("adult").to_string()));
    assert!(both
        .items
        .iter()
        .any(|item| item.book_id == id("pro").to_string()));

    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let pro_only = store
        .feed(Some(actor), Some(id("basic")), 20)
        .await
        .unwrap();
    assert!(pro_only
        .items
        .iter()
        .all(|item| item.book_id != id("adult").to_string()));
    assert!(pro_only
        .items
        .iter()
        .any(|item| item.book_id == id("pro").to_string()));
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    let revoked = store
        .feed(Some(actor), Some(id("basic")), 20)
        .await
        .unwrap();
    assert!(revoked
        .items
        .iter()
        .all(|item| !visible_restricted.contains(&item.book_id.parse::<i64>().unwrap())));

    sqlx::query(
        "UPDATE rec_runtime_policy SET active_generation=$1::uuid,previous_generation=$2::uuid,
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
}
