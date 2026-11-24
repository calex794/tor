//! Existing BR-060 fixtures: simulate the exact opens that precede Book save.
//! This is test setup, never a production path or historical backfill.
use sqlx::PgPool;

pub async fn mark_opened(pool: &PgPool, users: &[i64], chapters: &[i64]) {
    sqlx::query(
        "INSERT INTO reader_free_opened_editions(user_id,chapter_id,version_id)
         SELECT u.id,c.id,p.version_id FROM users u
         CROSS JOIN chapters c
         JOIN chapter_publications p ON p.chapter_id=c.id
         WHERE u.id=ANY($1::bigint[]) AND c.id=ANY($2::bigint[])
         ON CONFLICT(user_id,chapter_id) DO NOTHING",
    )
    .bind(users)
    .bind(chapters)
    .execute(pool)
    .await
    .unwrap();
}
