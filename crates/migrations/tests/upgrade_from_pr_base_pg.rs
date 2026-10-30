//! Verify an upgrade from the exact PR base schema, not just a fresh install.
//! Runs only on a disposable tor_test PostgreSQL database in CI. Never use a
//! production DB or any database other than tor_test.
use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use std::{env, path::Path};
use tor_migrations::repositories::MIGRATOR;

#[tokio::test]
#[ignore = "requires disposable tor_test and exact PR-base SQL migration files"]
async fn upgrade_from_pr_base_preserves_every_applied_checksum() {
    let baseline_dir = env::var("TOR_BASE_MIGRATIONS_DIR")
        .expect("CI must extract the exact GitHub pull-request base");
    let url =
        env::var("TOR_UPGRADE_DATABASE_URL").expect("CI must use an isolated tor_test database");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect isolated PostgreSQL 18");
    let (database, role): (String, String) =
        sqlx::query_as("SELECT current_database(), current_user")
            .fetch_one(&pool)
            .await
            .expect("identify the database and role");
    assert_eq!(database, "tor_test", "never mutate persistent databases");
    assert_eq!(role, "tor_migrator", "never run as the application role");
    let baseline = Migrator::new(Path::new(&baseline_dir))
        .await
        .expect("discover exact base SQL files");
    assert_eq!(
        baseline.migrations.len(),
        54,
        "exact main must have 54 SQLx migrations"
    );
    assert_eq!(
        MIGRATOR.migrations.len(),
        56,
        "merged main and G-08/C-18 must have 56"
    );
    assert!(
        MIGRATOR
            .migrations
            .iter()
            .any(|m| m.version == 202610300054)
            && MIGRATOR
                .migrations
                .iter()
                .any(|m| m.version == 202610310001),
        "both staged G-08 and C-18 migrations must exist"
    );
    assert!(
        MIGRATOR.migrations.iter().any(|m| {
            m.version
                < baseline
                    .migrations
                    .last()
                    .expect("baseline has migrations")
                    .version
                && !baseline
                    .migrations
                    .iter()
                    .any(|old| old.version == m.version)
        }),
        "the isolated upgrade must exercise an out-of-order pending version"
    );

    // A matching numeric version must keep its *original description* and
    // checksum. A renamed/applied old migration is not a safe upgrade.
    for old in baseline.migrations.iter() {
        let current = MIGRATOR
            .migrations
            .iter()
            .find(|m| m.version == old.version)
            .unwrap_or_else(|| {
                panic!(
                    "base migration version {} missing in the integration",
                    old.version
                )
            });
        assert_eq!(
            current.description, old.description,
            "migration description changed at version {}",
            old.version
        );
        assert_eq!(
            current.checksum, old.checksum,
            "applied SQLx checksum changed at version {}",
            old.version
        );
    }

    baseline
        .run(&pool)
        .await
        .expect("apply exact PR-base migrations");
    let before: Vec<(i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT version, description, checksum FROM _sqlx_migrations
         WHERE success = TRUE ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("list applied baseline checksums");
    assert_eq!(before.len(), baseline.migrations.len());

    // Additional Draft migrations are run *after* the highest already
    // applied main version: catch SQLx ordering errors and schema drift.
    MIGRATOR
        .run(&pool)
        .await
        .expect("upgrade from applied main to Draft");
    MIGRATOR
        .run(&pool)
        .await
        .expect("idempotent second upgrade");
    let after: Vec<(i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT version, description, checksum FROM _sqlx_migrations
         WHERE success = TRUE ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("verify upgraded SQLx history");
    assert_eq!(after.len(), MIGRATOR.migrations.len());
    for old in &before {
        assert!(
            after.contains(old),
            "old applied SQLx identity/checksum changed after upgrade: {}",
            old.0
        );
    }
    let failed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success = FALSE")
            .fetch_one(&pool)
            .await
            .expect("inspect failed migrations");
    assert_eq!(failed, 0, "no partially applied migrations");
    let read_at: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns
         WHERE table_schema='public'
           AND table_name='notification_event_recipient_intents'
           AND column_name='read_at')",
    )
    .fetch_one(&pool)
    .await
    .expect("probe C-18 read marker");
    assert!(read_at, "C-18 per-event read marker missing");
    let candidates: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.reader_staged_revision_candidates') IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("probe C-18 eligibility view");
    assert!(candidates, "C-18 eligibility view missing");
    pool.close().await;
}
