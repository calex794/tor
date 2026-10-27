//! F-03: Unicode normalization regression on isolated PostgreSQL.
use sqlx::PgPool;
use tor_search::repositories::PgSearch;

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL"]
async fn f_03_nfkc_and_yo_e_search_match_without_exposing_private_rows() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let pool = PgPool::connect(&url).await.unwrap();
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(db, "tor_ci", "never mutate live records");

    let mut visible_id = 0_i64;
    for (key, title, author, visibility, held, deleted) in [
        (
            "visible",
            "Ежовая история Starship 64",
            "Семен",
            "public",
            false,
            false,
        ),
        (
            "draft",
            "Ёжовая история Невидимая",
            "Сёмён",
            "draft",
            false,
            false,
        ),
        (
            "held",
            "Ёжовая история Заблокированная",
            "Сёмён",
            "public",
            true,
            false,
        ),
        (
            "deleted",
            "Ёжовая история Удалённая",
            "Сёмён",
            "public",
            false,
            true,
        ),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books (source_identity,target_language,title,author_name,genres,
                                visibility,admin_hold,is_deleted)
             VALUES ($1,'ru',$2,$3,ARRAY['f03-nfkc-test'],$4,$5,$6)
             ON CONFLICT (source_identity,target_language)
             DO UPDATE SET title=EXCLUDED.title,author_name=EXCLUDED.author_name,
                           genres=EXCLUDED.genres,visibility=EXCLUDED.visibility,
                           admin_hold=EXCLUDED.admin_hold,is_deleted=EXCLUDED.is_deleted
             RETURNING id",
        )
        .bind(format!("ci:f03:nfkc:{key}"))
        .bind(title)
        .bind(author)
        .bind(visibility)
        .bind(held)
        .bind(deleted)
        .fetch_one(&pool)
        .await
        .unwrap();
        if key == "visible" {
            visible_id = id;
        }
    }

    let search = PgSearch::new(pool.clone());
    for q in [
        "Ёжовая",
        "Ежовая",
        "Семён",
        "Семен",
        "Ｓｔａｒｓｈｉｐ",
        "６４",
    ] {
        let page = search
            .search_faceted_sorted(
                q,
                Some("ru"),
                Some("f03-nfkc-test"),
                None,
                20,
                Some("newest"),
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1, "unexpected normalized results for {q}");
        assert_eq!(page.items[0].id as i64, visible_id);
        assert_eq!(
            search
                .count_faceted(q, Some("ru"), Some("f03-nfkc-test"))
                .await,
            Ok(1)
        );
    }
    // Multiple words and lexical sorting must use the same normalized query.
    let lexical = search
        .search_faceted_sorted(
            "Ёжовая Семён",
            Some("ru"),
            Some("f03-nfkc-test"),
            None,
            20,
            Some("title"),
        )
        .await
        .unwrap();
    assert_eq!(lexical.items.len(), 1);
    assert_eq!(lexical.items[0].id as i64, visible_id);

    // Removing visibility immediately changes both the matches and exact total.
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(visible_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        search
            .count_faceted("Ёжовая", Some("ru"), Some("f03-nfkc-test"))
            .await,
        Ok(0)
    );
    let hidden = search
        .search_faceted_sorted(
            "Ёжовая",
            Some("ru"),
            Some("f03-nfkc-test"),
            None,
            20,
            Some("newest"),
        )
        .await
        .unwrap();
    assert!(
        hidden.items.is_empty(),
        "held, draft, deleted books must not leak"
    );
}
