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
        "INSERT INTO books(source_identity,target_language,title,original_title,alt_titles,visibility)
         VALUES($1,'ru',$2,$3,ARRAY[$4]::text[],'public') RETURNING id"
    )
    .bind(format!("s019:public:{suffix}"))
    .bind(&title).bind(&zh).bind(&en).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru',$2,'draft')"
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
