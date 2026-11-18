//! E-08 release upgrade regression against *only* a disposable PostgreSQL CI database.
//! Replay non-E-08 migration blobs except later migrations depending on
//! E-08 tables, then install E-08 and its dependent migrations with SQLx.
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

// Later opt-in email intents depend on canonical E-08 notifications.
// Keep this fixture's pre-E-08 baseline FK-complete.
const E08_DEPENDENT_VERSIONS: [i64; 1] = [202701198653];

async fn durable_catalogue_snapshot(
    pool: &sqlx::PgPool,
    book_id: i64,
    chapter_id: i64,
    version_id: i64,
    pack_id: i64,
) -> Result<String, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
           'book',(SELECT jsonb_build_object('id',id,'source',source_identity)
                   FROM books WHERE id=$1),
           'chapter',(SELECT jsonb_build_object('id',id,'book',book_id,'number',chapter_no)
                      FROM chapters WHERE id=$2),
           'version',(SELECT jsonb_build_object('id',id,'chapter',chapter_id,
                        'body',body_markdown,'digest',encode(content_sha256,'hex'))
                      FROM chapter_versions WHERE id=$3),
           'single',(SELECT jsonb_build_object('key',object_key,'bucket',bucket,
                         'digest',encode(sha256,'hex'),'size',size_bytes)
                      FROM free_chapter_storage_objects
                      WHERE chapter_id=$2 AND version_id=$3),
           'pack',(SELECT jsonb_build_object('id',id,'key',object_key,'bucket',bucket,
                       'digest',encode(pack_sha256,'hex'))
                   FROM free_storage_packs WHERE id=$4),
           'generation',(SELECT jsonb_build_object('generation',generation,
                              'pack',pack_id,'frame_offset',frame_offset,
                              'frame_bytes',frame_bytes)
                         FROM free_edition_storage_generations
                         WHERE chapter_id=$2 AND version_id=$3 AND pack_id=$4)
         )::text",
    )
    .bind(book_id)
    .bind(chapter_id)
    .bind(version_id)
    .bind(pack_id)
    .fetch_one(pool)
    .await
}

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
                .filter(|migration| {
                    !E08_VERSIONS.contains(&migration.version)
                        && !E08_DEPENDENT_VERSIONS.contains(&migration.version)
                })
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    assert_eq!(
        MIGRATOR.migrations.len(),
        baseline.migrations.len() + E08_VERSIONS.len() + E08_DEPENDENT_VERSIONS.len()
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
    for version in E08_VERSIONS.into_iter().chain(E08_DEPENDENT_VERSIONS) {
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

    // FND-01: the installed catalogue must survive an actual SQLx ledger
    // upgrade. These are synthetic metadata receipts: NO S3 is contacted.
    let book_id: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE source_identity='ci:e08:installed-upgrade'")
            .fetch_one(&pool)
            .await?;
    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title)
         VALUES($1,1,'Existing chapter before upgrade') RETURNING id",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await?;
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,'durable free body',sha256('durable free body'::bytea)) RETURNING id",
    )
    .bind(chapter_id)
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO free_chapter_storage_objects
         (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
         VALUES($1,$2,'tor-ci-fnd01-storage',$3,sha256('durable free body'::bytea),17)",
    )
    .bind(chapter_id)
    .bind(version_id)
    .bind(format!(
        "private/free-chapters/{chapter_id}/{version_id}.md"
    ))
    .execute(&pool)
    .await?;
    let pack_id: i64 = sqlx::query_scalar(
        "INSERT INTO free_storage_packs
         (bucket,object_key,pack_sha256,manifest_key,manifest_sha256,
          pack_bytes,manifest_bytes,seek_table_start)
         VALUES('tor-ci-fnd01-storage',
                'private/free-packs/ci/fnd01-installed-upgrade.zst',
                sha256('fnd01-pack'::bytea),
                'private/free-packs/ci/fnd01-installed-upgrade.manifest.json',
                sha256('fnd01-manifest'::bytea),150,30,120) RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO free_edition_storage_generations
         (chapter_id,version_id,generation,placement_kind,
          content_sha256,size_bytes,pack_id,frame_offset,frame_bytes)
         VALUES($1,$2,1,'pack',sha256('durable free body'::bytea),17,$3,0,20)",
    )
    .bind(chapter_id)
    .bind(version_id)
    .bind(pack_id)
    .execute(&pool)
    .await?;
    let catalogue_before =
        durable_catalogue_snapshot(&pool, book_id, chapter_id, version_id, pack_id).await?;
    assert!(catalogue_before.contains("private/free-chapters/"));
    assert!(catalogue_before.contains("private/free-packs/"));

    MIGRATOR.run(&pool).await?;
    MIGRATOR.run(&pool).await?; // no replay or accidental checksum repair
    assert_eq!(
        durable_catalogue_snapshot(&pool, book_id, chapter_id, version_id, pack_id).await?,
        catalogue_before,
        "additive SQLx upgrade must preserve Book, ChapterVersion and exact S3 references"
    );

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
            assert!(
                E08_VERSIONS.contains(&version) || E08_DEPENDENT_VERSIONS.contains(&version),
                "unexpected new {version}"
            );
        }
    }
    // The dependent intent table must be installed after its canonical parent.
    let notification_fk_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint
         WHERE conrelid='reader_email_notification_intents'::regclass
           AND confrelid='user_notifications'::regclass AND contype='f'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(notification_fk_count, 1);

    let preserved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM books WHERE source_identity='ci:e08:installed-upgrade'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(preserved, 1, "E-08 upgrade changed existing public data");

    pool.close().await;
    Ok(())
}
