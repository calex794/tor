//! S019: title hints across RU/EN/ZH, strict public-only visibility.
use sqlx::PgPool;
use tor_books::repositories::postgres::PgAuthoring;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn s019_preflight_matches_public_aliases_but_never_private_drafts() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let zh = format!("修仙之路{suffix}");
    let en = format!("The Path to Immortality {suffix}");
    let title = format!("Путь бессмертного {suffix}");
    let hidden = format!("Секретная глава {suffix}");
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,original_title,alt_titles,visibility,access_level)
         VALUES($1,'ru',$2,$3,ARRAY[$4]::text[],'public','basic') RETURNING id"
    )
    .bind(format!("s019:public:{suffix}"))
    .bind(&title).bind(&zh).bind(&en).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru',$2,'draft')",
    )
    .bind(format!("s019:private:{suffix}"))
    .bind(&hidden)
    .execute(&pool)
    .await
    .unwrap();

    let repo = PgAuthoring::new(pool.clone());
    for candidate in [&zh, &en, &title] {
        let found = repo
            .public_title_suggestions("ru", &[candidate])
            .await
            .unwrap();
        assert!(found.iter().any(|book| book.id == id), "{candidate}");
        assert!(found.iter().all(|book| book.id > 0));
    }
    sqlx::query("UPDATE books SET visibility_status='hidden' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    for candidate in [&zh, &en, &title] {
        let found = repo
            .public_title_suggestions("ru", &[candidate])
            .await
            .unwrap();
        assert!(
            !found.iter().any(|book| book.id == id),
            "moderated Book leaked via title suggestions: {candidate}"
        );
    }
    sqlx::query("UPDATE books SET visibility_status='active' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    // S020: title hints are anonymous/basic discovery even when the
    // registration form itself requires a signed-in account. Distinct B age
    // grants do not turn this public S019 projection into an actor feed.
    for level in [Some("adult"), Some("pro_adult"), None] {
        sqlx::query("UPDATE books SET access_level=$1 WHERE id=$2")
            .bind(level)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        for term in [&zh, &en, &title] {
            let suggestions = repo
                .public_title_suggestions("ru", &[term])
                .await
                .unwrap();
            assert!(
                suggestions.iter().all(|book| book.id != id),
                "restricted / legacy Book title exposed at {level:?}: {term}"
            );
        }
    }
    sqlx::query("UPDATE books SET access_level='basic', lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let pending = repo.public_title_suggestions("ru", &[&title]).await.unwrap();
    assert!(!pending.iter().any(|book| book.id == id));
    sqlx::query("UPDATE books SET lifecycle_status='in_progress' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo
        .public_title_suggestions("ru", &[&title])
        .await
        .unwrap()
        .iter()
        .any(|book| book.id == id));

    let other = repo.public_title_suggestions("en", &[&zh]).await.unwrap();
    assert!(!other.iter().any(|book| book.id == id));
    let forbidden = repo
        .public_title_suggestions("ru", &[&hidden])
        .await
        .unwrap();
    assert!(!forbidden.iter().any(|book| book.title == hidden));
    let empty = repo.public_title_suggestions("ru", &["ab"]).await.unwrap();
    assert!(empty.is_empty());
    assert!(repo
        .public_title_suggestions("ru;DROP", &[&zh])
        .await
        .is_err());
    pool.close().await;
}
