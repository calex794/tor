//! E-08 release upgrade regression against *only* a disposable PostgreSQL CI database.
//! Replay the exact checked-out non-E-08 migration blobs as the installed
//! baseline, then install the fourteen Draft extensions with SQLx's checksum ledger.
//! The PR CI checkout is a synthetic merge with the newest main: baseline
//! migrations can grow while an E-08 PR is being reviewed. Never touch retained data.

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use std::{borrow::Cow, collections::HashMap, env, error::Error, time::Duration};
use tor_migrations::repositories::MIGRATOR;

const E08_VERSIONS: [i64; 14] = [
    202610310295,
    202610310296,
    202610310300,
    202611010010,
    202611010020,
    202611010030,
    202611010050,
    202611010060,
    202611014287,
    202611014931,
    202611019237,
    202611020527,
    202611027613,
    202611029083,
];

#[tokio::test]
#[ignore = "requires a disposable tor_ci_e08_upgrade database owned by tor_migrator"]
async fn e08_upgrade_retains_installed_main_sqlx_ledger_and_preexisting_books(
) -> Result<(), Box<dyn Error>> {
    let url = env::var("E08_UPGRADE_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await?;
    let (db, role): (String, String) =
        sqlx::query_as("SELECT current_database()::text, current_user::text")
            .fetch_one(&pool)
            .await?;
    assert_eq!(db, "tor_ci_e08_upgrade", "never use a retained database");
    assert_eq!(role, "tor_migrator", "never run as a web or superuser role");

    // Compare against real SQLx-embedded bytes and checksums, not a second
    // hand-maintained SQL file or a plain psql replay with no ledger.
    let baseline = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| !E08_VERSIONS.contains(&migration.version))
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    assert_eq!(
        MIGRATOR.migrations.len(),
        baseline.migrations.len() + E08_VERSIONS.len()
    );
    // Preserve at least the verified historical main baseline and its final
    // refund migration. A PR checkout may also include newer main migrations
    // (for example 202610310293) without changing any old SQL bytes.
    assert!(baseline.migrations.len() >= 67);
    assert!(baseline.version_exists(202610310291));
    assert!(baseline
        .migrations
        .last()
        .is_some_and(|migration| migration.version >= 202610310291));
    for version in E08_VERSIONS {
        assert!(MIGRATOR.version_exists(version));
        assert!(!baseline.version_exists(version));
    }

    baseline.run(&pool).await?;
    let installed_before: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await?;
    assert_eq!(installed_before.len(), baseline.migrations.len());
    assert!(installed_before.iter().all(|(_, _, success)| *success));
    let before_checksums: HashMap<_, _> = installed_before
        .into_iter()
        .map(|(version, checksum, _)| (version, checksum))
        .collect();

    // A representative existing Book must survive the incremental upgrade.
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:e08:installed-upgrade','ru','Existing before E-08','public')",
    )
    .execute(&pool)
    .await?;

    MIGRATOR.run(&pool).await?;
    MIGRATOR.run(&pool).await?; // no replay or accidental checksum repair

    let installed_after: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await?;
    assert_eq!(installed_after.len(), MIGRATOR.migrations.len());
    assert!(installed_after.iter().all(|(_, _, success)| *success));
    for (version, checksum, _) in installed_after {
        if let Some(old_checksum) = before_checksums.get(&version) {
            assert_eq!(&checksum, old_checksum, "checksum changed for {version}");
        } else {
            assert!(E08_VERSIONS.contains(&version), "unexpected new {version}");
        }
    }
    let preserved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM books WHERE source_identity='ci:e08:installed-upgrade'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(preserved, 1, "E-08 upgrade changed existing public data");

    pool.close().await;
    Ok(())
}
