//! E-08 release upgrade regression against *only* a disposable PostgreSQL CI database.
//! First replay the exact 67 main migration blobs, then install the six Draft
//! extensions using SQLx's real version/checksum ledger (including 200/201
//! below the previously installed 291). Never execute against retained data.

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use std::{borrow::Cow, collections::HashMap, env, error::Error, time::Duration};
use tor_migrations::repositories::MIGRATOR;

const E08_VERSIONS: [i64; 6] = [
    202610310200,
    202610310201,
    202610310300,
    202611010010,
    202611010020,
    202611010030,
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
    assert_eq!(MIGRATOR.migrations.len(), baseline.migrations.len() + E08_VERSIONS.len());
    assert_eq!(baseline.migrations.len(), 67);
    assert_eq!(
        baseline
            .migrations
            .last()
            .map(|migration| migration.version),
        Some(202610310291)
    );
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
