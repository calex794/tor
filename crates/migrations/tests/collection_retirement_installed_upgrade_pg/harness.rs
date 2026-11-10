//! Real SQLx checksums and strict disposable CI database boundaries.
use super::fixtures::{run, TestResult};
use sqlx::{
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::{borrow::Cow, collections::HashMap, env, str::FromStr, time::Duration};
use tor_migrations::repositories::MIGRATOR;

pub const PRIMITIVES: i64 = 202611163274;
pub const BARRIER: i64 = 202611168903;

pub fn through(version: i64) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version <= version)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
}

pub async fn connect(
    variable: &str,
    database: &str,
    role: &str,
    connections: u32,
) -> TestResult<PgPool> {
    let url = env::var(variable)?;
    let options = PgConnectOptions::from_str(&url)?;
    assert_eq!(options.get_host(), "127.0.0.1", "loopback CI only");
    assert_eq!(options.get_port(), 5432);
    assert_eq!(options.get_database(), Some(database));
    assert_eq!(options.get_username(), role);
    let pool = PgPoolOptions::new()
        .max_connections(connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await?;
    let actual: (String, String) =
        sqlx::query_as("SELECT current_database()::text,current_user::text")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        actual,
        (database.to_owned(), role.to_owned()),
        "never a production or superuser connection"
    );
    Ok(pool)
}

pub async fn grant_app(pool: &PgPool) -> TestResult {
    let database: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(pool)
        .await?;
    assert!([
        "tor_ci_collection_retirement_upgrade",
        "tor_ci_collection_retirement_startup"
    ]
    .contains(&database.as_str()));
    if database == "tor_ci_collection_retirement_upgrade" {
        // CI revokes PUBLIC database privileges. This grant exists only to
        // exercise an app-role temp-shadow attack in this disposable DB.
        run(
            pool,
            "GRANT TEMP ON DATABASE tor_ci_collection_retirement_upgrade TO tor_app",
        )
        .await?;
    }
    run(pool,
        "GRANT USAGE ON SCHEMA public TO tor_app;
         GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO tor_app;
         GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO tor_app;
         ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO tor_app;
         ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT USAGE,SELECT ON SEQUENCES TO tor_app"
    ).await
}

pub async fn baseline(pool: &PgPool) -> TestResult<HashMap<i64, Vec<u8>>> {
    let baseline = through(PRIMITIVES - 1);
    assert_eq!(baseline.migrations.len(), 190);
    assert!(baseline.version_exists(202611158256));
    baseline.run(pool).await?;
    let installed: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await?;
    assert_eq!(installed.len(), 190);
    assert!(installed.iter().all(|row| row.2));
    Ok(installed
        .into_iter()
        .map(|(version, checksum, _)| (version, checksum))
        .collect())
}

pub async fn assert_ledger(pool: &PgPool, before: &HashMap<i64, Vec<u8>>) -> TestResult {
    let installed: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await?;
    assert_eq!(installed.len(), 192);
    assert!(installed.iter().all(|row| row.2));
    for (version, checksum, _) in installed {
        if let Some(old) = before.get(&version) {
            assert_eq!(&checksum, old, "old checksum changed for {version}");
        } else {
            assert!([PRIMITIVES, BARRIER].contains(&version));
        }
    }
    Ok(())
}
