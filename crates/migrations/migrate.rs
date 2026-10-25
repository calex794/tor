//! Run manually as tor_migrator after taking a backup; never from tor-web.
use sqlx::postgres::PgPoolOptions;
use std::{env, error::Error, time::Duration};
use tor_migrations::repositories::MIGRATOR;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = env::var("DATABASE_URL")?;
    let pool = PgPoolOptions::new().max_connections(1)
        .acquire_timeout(Duration::from_secs(5)).connect(&url).await?;
    let role: String = sqlx::query_scalar("SELECT current_user").fetch_one(&pool).await?;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool).await?;
    if role != "tor_migrator" {
        return Err("TOR migrations require the dedicated tor_migrator role".into());
    }
    if !matches!(database.as_str(), "tor" | "tor_ci" | "tor_test") {
        return Err("Refusing to migrate an unapproved database".into());
    }
    MIGRATOR.run(&pool).await?;
    println!("TOR SQLx migrations applied and verified");
    pool.close().await;
    Ok(())
}
