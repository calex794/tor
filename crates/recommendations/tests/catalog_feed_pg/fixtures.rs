use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_recommendations::{domain::feedback::Feed, repositories::PgRecommendations};

pub struct Fixture {
    pub pool: PgPool,
    pub store: PgRecommendations,
    pub actors: [i64; 2],
    pub books: Vec<i64>,
}

impl Fixture {
    pub async fn new(name: &str, pool: PgPool) -> Self {
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
        }
    }

    pub async fn cleanup(self) {
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

pub fn ids(feed: &Feed) -> Vec<i64> {
    feed.items
        .iter()
        .map(|item| item.book_id.parse().unwrap())
        .collect()
}

pub fn assert_catalog(feed: &Feed) {
    assert!(feed.generation_id.is_none() && feed.next_cursor.is_none());
    assert!(!feed.personalized);
    assert!(feed.items.iter().all(|item| {
        item.source == "catalog"
            && item.reason == "new_release"
            && item.algorithm_version == "catalog_new_releases_v1"
            && item.ticket.is_none()
    }));
}
