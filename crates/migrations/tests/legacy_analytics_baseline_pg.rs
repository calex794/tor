//! An explicit pre-retirement database for retained collector regressions.
//! Current-schema business/HTTP tests must never use this database.
use sqlx::{
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{borrow::Cow, env, error::Error, str::FromStr, time::Duration};
use tor_migrations::repositories::MIGRATOR;

const LEGACY_LAST_VERSION: i64 = 202611163274;
const RETIREMENT_VERSION: i64 = 202611168903;

#[tokio::test]
#[ignore = "requires disposable loopback tor_ci_legacy_analytics owned by tor_migrator"]
async fn prepare_explicit_legacy_analytics_baseline_with_sqlx_ledger() -> Result<(), Box<dyn Error>> {
    let url = env::var("LEGACY_ANALYTICS_MIGRATOR_URL")?;
    let options = PgConnectOptions::from_str(&url)?;
    assert_eq!(options.get_host(), "127.0.0.1", "loopback fixtures only");
    assert_eq!(options.get_port(), 5432);
    assert_eq!(options.get_database(), Some("tor_ci_legacy_analytics"));
    assert_eq!(options.get_username(), "tor_migrator");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await?;
    let identity: (String, String) =
        sqlx::query_as("SELECT current_database()::text,current_user::text")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        identity,
        ("tor_ci_legacy_analytics".into(), "tor_migrator".into()),
        "never modify the current application or upgrade fixture database"
    );

    let baseline = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version < RETIREMENT_VERSION)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    assert_eq!(baseline.migrations.len(), 191);
    assert_eq!(
        baseline.migrations.last().unwrap().version,
        LEGACY_LAST_VERSION
    );
    assert!(MIGRATOR.version_exists(RETIREMENT_VERSION));
    assert!(!baseline.version_exists(RETIREMENT_VERSION));

    // Match the disposable CI bootstrap: application access is restricted to
    // ordinary data operations; schema creation remains with tor_migrator.
    sqlx::raw_sql(
        "GRANT USAGE ON SCHEMA public TO tor_app;
         ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
         ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE,SELECT ON SEQUENCES TO tor_app;",
    )
    .execute(&pool)
    .await?;

    // SQLx refuses a database already containing a later version; do not use
    // ignore_missing, change checksums, replay ad hoc SQL or disable triggers.
    baseline.run(&pool).await?;
    baseline.run(&pool).await?;
    let installed: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await?;
    assert_eq!(installed.len(), baseline.migrations.len());
    for ((version, checksum, success), migration) in installed.iter().zip(baseline.iter()) {
        assert!(*success);
        assert_eq!(*version, migration.version);
        assert_eq!(checksum.as_slice(), migration.checksum.as_ref());
    }
    let fenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations
         WHERE version >= $1 AND success)",
    )
    .bind(RETIREMENT_VERSION)
    .fetch_one(&pool)
    .await?;
    assert!(
        !fenced,
        "legacy collectors cannot run behind the retirement barrier"
    );
    let invalid_baseline: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.rec_analytics_invalidations') IS NULL
         OR to_regprocedure('public.rec_retire_analytics_batch(integer)') IS NOT NULL
         OR EXISTS(SELECT 1 FROM pg_trigger
           WHERE tgname='retired_collection_write_fence')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        !invalid_baseline,
        "legacy baseline requires the primitives and no installed barrier objects"
    );
    println!("TOR_LEGACY_ANALYTICS_SQLX_OK 191/191 database=tor_ci_legacy_analytics barrier=absent");
    pool.close().await;
    Ok(())
}
