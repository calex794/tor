use sqlx::PgPool;
use tor_recommendations::repositories::PgRecommendations;

pub const STRIDE: i64 = 1_000_000_000;

pub async fn create(pool: PgPool, name: &str) -> (PgPool, PgRecommendations, i64, i64) {
    let email = format!("explicit-pref-{name}@tor-ci.example");
    sqlx::query("DELETE FROM users WHERE email_normalized=$1")
        .bind(&email)
        .execute(&pool)
        .await
        .unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Explicit preferences','$argon2id$test') RETURNING id",
    )
    .bind(email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Explicit preference book','public','basic')
         ON CONFLICT(source_identity,target_language) DO UPDATE SET
           visibility='public',visibility_status='active',access_level='basic',
           admin_hold=false,is_deleted=false,created_at=now()
         RETURNING id",
    )
    .bind(format!("ci:explicit-pref:{name}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let store = PgRecommendations::new(pool.clone());
    (pool, store, actor, book)
}

pub async fn counts(pool: &PgPool, actor: i64) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM rec_preference_accounts WHERE user_id=$1),
           (SELECT count(*) FROM rec_book_preferences WHERE user_id=$1),
           (SELECT count(*) FROM rec_user_preference_commands WHERE user_id=$1),
           (SELECT count(*) FROM rec_subjects WHERE user_id=$1)",
    )
    .bind(actor)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn preference(pool: &PgPool, actor: i64, book: i64) -> Option<(bool, i64, String)> {
    sqlx::query_as(
        "SELECT hidden,revision,expires_at::text FROM rec_book_preferences
         WHERE user_id=$1 AND book_id=$2",
    )
    .bind(actor)
    .bind(book)
    .fetch_optional(pool)
    .await
    .unwrap()
}

pub async fn epoch(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar("SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id=$1")
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap()
}
