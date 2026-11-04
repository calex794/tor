//! S020/F-02: authenticated discoverability is not anonymous SEO/public search.
//! No production database, deployment or migration. Disposable tor_ci only.
use sqlx::PgPool;
use tor_search::{
    repositories::{showcases::PgShowcases, CatalogFacets, PgSearch},
    services::showcases::ShowcaseRepository,
};

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending in /tor_ci"]
async fn s020_actor_catalog_age_is_live_scoped_and_cursors_remain_exact() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated postgres");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci");

    let mark = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let term = format!("S020CatalogAge{mark}");
    let genre = format!("s020-age-genre-{mark}");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'S020 catalog viewer','$argon2id$test') RETURNING id",
    )
    .bind(format!("s020-catalog-{mark}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut digest = [0u8; 32];
    digest[..16].copy_from_slice(&mark.to_be_bytes());
    digest[16..].copy_from_slice(&mark.to_be_bytes());
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();

    let mut ids = Vec::new();
    for (suffix, tier, visibility, held) in [
        ("basic", Some("basic"), "public", false),
        ("adult", Some("adult"), "public", false),
        ("pro", Some("pro_adult"), "public", false),
        ("unknown", None, "public", false),
        ("draft", Some("basic"), "draft", false),
        ("held", Some("basic"), "public", true),
        ("pending_basic", Some("basic"), "public", false),
        ("pending_adult", Some("adult"), "public", false),
        ("pending_pro", Some("pro_adult"), "public", false),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,author_name,
                              visibility,access_level,admin_hold,genres,tags,fandoms)
             VALUES($1,'ru',$2,$3,$4,$5,$6,
                    ARRAY[$7::text],ARRAY['s020-age-tag'],ARRAY['s020-age-fandom'])
             RETURNING id",
        )
        .bind(format!("ci:s020:catalog:{mark}:{suffix}"))
        .bind(format!("{term}-{suffix}"))
        .bind(format!("Author{mark}-{suffix}"))
        .bind(visibility)
        .bind(tier)
        .bind(held)
        .bind(&genre)
        .fetch_one(&pool)
        .await
        .unwrap();
        ids.push((suffix, id as u64));
    }
    let id = |name: &str| ids.iter().find(|(n, _)| *n == name).unwrap().1;
    for label in ["pending_basic", "pending_adult", "pending_pro"] {
        sqlx::query("UPDATE books SET lifecycle_status=\'pending_deletion\' WHERE id=$1")
            .bind(id(label) as i64)
            .execute(&pool)
            .await
            .unwrap();
    }
    let facets = CatalogFacets {
        term: &term,
        language: Some("ru"),
        genre: Some(&genre),
        tag: Some("s020-age-tag"),
        fandom: Some("s020-age-fandom"),
    };
    let public = PgSearch::new(pool.clone());
    let viewer = public.with_session_digest(Some(digest));
    let invalid = public.with_session_digest(Some([0xAB; 32]));

    async fn found(search: &PgSearch, facets: CatalogFacets<'_>) -> Vec<u64> {
        search
            .browse_catalog_page(facets, None, None, 20, Some("oldest"))
            .await
            .unwrap()
            .items
            .into_iter()
            .map(|b| b.id)
            .collect()
    }
    assert_eq!(found(&public, facets).await, vec![id("basic")]);
    assert_eq!(found(&invalid, facets).await, vec![id("basic")]);
    assert_eq!(found(&viewer, facets).await, vec![id("basic")]);
    assert_eq!(viewer.count_catalog(facets).await, Ok(1));

    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(found(&viewer, facets).await, vec![id("basic"), id("adult")]);
    assert_eq!(viewer.count_catalog(facets).await, Ok(2));
    sqlx::query(
        "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES($1,'pro_adult','moderation',TRUE)",
    )
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        found(&viewer, facets).await,
        vec![id("basic"), id("adult"), id("pro")]
    );
    assert_eq!(viewer.count_catalog(facets).await, Ok(3));
    // F-04 uses the same anonymous indexability contract as F-02.
    let showcased = PgShowcases::new(pool.clone())
        .latest_public(Some(&genre), 24)
        .await
        .unwrap();
    assert_eq!(
        showcased.into_iter().map(|book| book.id).collect::<Vec<_>>(),
        vec![id("basic")],
        "pending_deletion must not leak to anonymous F-04 showcases"
    );
    assert_eq!(found(&public, facets).await, vec![id("basic")]);

    for sort in ["oldest", "newest", "title", "author"] {
        let full = viewer
            .browse_catalog_page(facets, None, None, 20, Some(sort))
            .await
            .unwrap()
            .items;
        assert_eq!(full.len(), 3, "{sort} all accessible");
        let first = viewer
            .browse_catalog_page(facets, None, None, 1, Some(sort))
            .await
            .unwrap();
        let second = viewer
            .browse_catalog_page(facets, first.next_cursor.as_deref(), None, 1, Some(sort))
            .await
            .unwrap();
        let third = viewer
            .browse_catalog_page(facets, second.next_cursor.as_deref(), None, 1, Some(sort))
            .await
            .unwrap();
        assert_eq!(
            [
                first.items.clone(),
                second.items.clone(),
                third.items.clone()
            ]
            .concat(),
            full,
            "{sort} forward pagination"
        );
        let back = viewer
            .browse_catalog_page(
                facets,
                None,
                third.previous_cursor.as_deref(),
                1,
                Some(sort),
            )
            .await
            .unwrap();
        assert_eq!(back.items, second.items, "{sort} reverse pagination");
    }

    // The 21+ grant independently permits pro_adult after 18+ is disabled.
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(found(&viewer, facets).await, vec![id("basic"), id("pro")]);
    assert_eq!(viewer.count_catalog(facets).await, Ok(2));
    sqlx::query("UPDATE user_level_grants SET is_active=FALSE WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(found(&viewer, facets).await, vec![id("basic")]);
    assert_eq!(viewer.count_catalog(facets).await, Ok(1));

    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(found(&viewer, facets).await, vec![id("basic")]);
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
