//! REC/FLOW v2 P1: installed SQLx upgrade, exact choices and safe legacy cutover.
//! This test never runs against the application tor_ci database or a retained DB.

use sqlx::{migrate::Migrator, postgres::PgPoolOptions, PgPool};
use std::{borrow::Cow, collections::HashMap, env, error::Error, time::Duration};
use tor_migrations::repositories::MIGRATOR;

const PREFERENCE_VERSION: i64 = 202611158256;

// This historical installed-upgrade proof ends at the accepted P1 boundary.
// Later retirement migrations intentionally reject the old consent UPDATE
// exercised below. Current P1 product behavior is tested on the latest schema.
fn preference_boundary() -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version <= PREFERENCE_VERSION)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
}
type PreferenceRow = (i64, bool, i64, String);
type CommandRow = (i64, Vec<u8>, Vec<u8>, i64, String, String);

async fn preferences(pool: &PgPool, actor: i64) -> Result<Vec<PreferenceRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT book_id,hidden,revision,expires_at::text FROM rec_book_preferences
         WHERE user_id=$1 ORDER BY book_id",
    )
    .bind(actor)
    .fetch_all(pool)
    .await
}

async fn commands(pool: &PgPool, actors: &[i64]) -> Result<Vec<CommandRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id,command_key,payload_hash,result_revision,command_type,created_at::text
         FROM rec_user_preference_commands WHERE user_id=ANY($1) ORDER BY user_id,command_key",
    )
    .bind(actors)
    .fetch_all(pool)
    .await
}

fn assert_legacy_fence(error: sqlx::Error) {
    let database = error.as_database_error().expect("database refusal");
    assert_eq!(database.code().as_deref(), Some("55000"));
}

#[tokio::test]
#[ignore = "requires disposable tor_ci_rec_preferences_upgrade owned by tor_migrator"]
async fn installed_preferences_upgrade_preserves_choices_and_fences_old_writers(
) -> Result<(), Box<dyn Error>> {
    let url = env::var("REC_PREFERENCES_UPGRADE_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await?;
    let (db, role): (String, String) =
        sqlx::query_as("SELECT current_database()::text,current_user::text")
            .fetch_one(&pool)
            .await?;
    assert_eq!(db, "tor_ci_rec_preferences_upgrade", "disposable DB only");
    assert_eq!(role, "tor_migrator", "never use the app or superuser role");

    let baseline = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version < PREFERENCE_VERSION)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    assert!(baseline.version_exists(202611144010));
    assert!(!baseline.version_exists(PREFERENCE_VERSION));
    assert!(MIGRATOR.version_exists(PREFERENCE_VERSION));
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

    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('rec-upgrade-reader@tor-ci.example','Existing reader','$argon2id$test') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    let command_only: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('rec-upgrade-command@tor-ci.example','Command only','$argon2id$test') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    let actors = [actor, command_only];
    let mut books = Vec::new();
    for name in ["hidden", "clear", "expired"] {
        let book: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES($1,'ru','Existing preference book','public','basic') RETURNING id",
        )
        .bind(format!("ci:rec-preference-upgrade:{name}"))
        .fetch_one(&pool)
        .await?;
        books.push(book);
    }
    sqlx::query(
        "INSERT INTO rec_subjects(user_id,enabled,revision,enabled_at,feedback_epoch)
         VALUES($1,true,13,now()-interval '15 days',7)",
    )
    .bind(actor)
    .execute(&pool)
    .await?;
    for (book, hidden, revision, offset) in [
        (books[0], true, 4_i64, "3 days"),
        (books[1], false, 9_i64, "1 day"),
        (books[2], true, 2_i64, "-2 hours"),
    ] {
        sqlx::query(
            "INSERT INTO rec_feedback_state(subject_id,book_id,hidden,revision,expires_at)
             SELECT subject_id,$2,$3,$4,now()+$5::interval FROM rec_subjects WHERE user_id=$1",
        )
        .bind(actor)
        .bind(book)
        .bind(hidden)
        .bind(revision)
        .bind(offset)
        .execute(&pool)
        .await?;
    }
    for (user, byte, kind, revision, age) in [
        (actor, 17_u8, "feedback", 7_000_000_004_i64, "5 days"),
        (actor, 34, "consent", 13, "7 days"),
        (actor, 51, "feedback", 7_000_000_002, "31 days"),
        (command_only, 68, "feedback", 1, "2 days"),
    ] {
        sqlx::query(
            "INSERT INTO rec_preference_commands(
                 user_id,command_key,payload_hash,result_revision,command_type,created_at)
             VALUES($1,$2,$3,$4,$5,now()-$6::interval)",
        )
        .bind(user)
        .bind(vec![byte; 32])
        .bind(vec![byte + 1; 32])
        .bind(revision)
        .bind(kind)
        .bind(age)
        .execute(&pool)
        .await?;
    }

    // A legacy transaction that already owns the old table must finish before
    // the migration snapshots it. No sleep-based assumption about its timing.
    let mut legacy = pool.begin().await?;
    sqlx::query(
        "UPDATE rec_feedback_state SET revision=10,expires_at=now()+interval '2 days'
         WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=$1) AND book_id=$2",
    )
    .bind(actor)
    .bind(books[1])
    .execute(&mut *legacy)
    .await?;
    let expected_preferences: Vec<PreferenceRow> = sqlx::query_as(
        "SELECT f.book_id,f.hidden,f.revision,f.expires_at::text
         FROM rec_feedback_state f JOIN rec_subjects s ON s.subject_id=f.subject_id
         WHERE s.user_id=$1 ORDER BY f.book_id",
    )
    .bind(actor)
    .fetch_all(&mut *legacy)
    .await?;
    let expected_commands: Vec<CommandRow> = sqlx::query_as(
        "SELECT user_id,command_key,payload_hash,result_revision,command_type,created_at::text
         FROM rec_preference_commands WHERE user_id=ANY($1) ORDER BY user_id,command_key",
    )
    .bind(actors.as_slice())
    .fetch_all(&mut *legacy)
    .await?;
    let upgrading_pool = pool.clone();
    let upgrade = tokio::spawn(async move { preference_boundary().run(&upgrading_pool).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks
                 WHERE relation='rec_feedback_state'::regclass
                   AND mode='ShareRowExclusiveLock' AND NOT granted)",
            )
            .fetch_one(&pool)
            .await?;
            if waiting {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    legacy.commit().await?;
    upgrade.await??;
    preference_boundary().run(&pool).await?;

    assert_eq!(preferences(&pool, actor).await?, expected_preferences);
    assert_eq!(commands(&pool, &actors).await?, expected_commands);
    let epochs: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id,feedback_epoch FROM rec_preference_accounts
         WHERE user_id=ANY($1) ORDER BY user_id",
    )
    .bind(actors.as_slice())
    .fetch_all(&pool)
    .await?;
    assert_eq!(epochs, vec![(actor, 7), (command_only, 0)]);
    let visible: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rec_book_preferences WHERE user_id=$1 AND hidden AND expires_at>now()",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await?;
    assert_eq!(visible, 1, "clear and expired choices cannot become hidden");
    let old_rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM rec_feedback_state)
              +(SELECT count(*) FROM rec_preference_commands)",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(old_rows, 0, "only one authoritative preference store");

    let installed_after: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        installed_after.len(),
        preference_boundary().migrations.len()
    );
    assert_eq!(installed_after.len(), 190);
    assert!(installed_after.iter().all(|(_, _, success)| *success));
    for (version, checksum, _) in installed_after {
        if let Some(before) = before_checksums.get(&version) {
            assert_eq!(&checksum, before, "old checksum changed for {version}");
        } else {
            assert_eq!(version, PREFERENCE_VERSION);
        }
    }

    // Statement fences reject old updates even after the source tables empty.
    for statement in [
        "INSERT INTO rec_feedback_state(subject_id,book_id,hidden,revision)
         SELECT s.subject_id,p.book_id,true,1 FROM rec_subjects s
         JOIN rec_book_preferences p ON p.user_id=s.user_id WHERE s.user_id=$1 LIMIT 1",
        "UPDATE rec_feedback_state SET hidden=false
         WHERE subject_id=(SELECT subject_id FROM rec_subjects WHERE user_id=$1)",
        "INSERT INTO rec_preference_commands(
             user_id,command_key,payload_hash,result_revision,command_type)
         VALUES($1,decode(repeat('ab',32),'hex'),decode(repeat('cd',32),'hex'),1,'feedback')",
        "INSERT INTO rec_preference_commands(
             user_id,command_key,payload_hash,result_revision,command_type)
         VALUES($1,decode(repeat('ef',32),'hex'),decode(repeat('ab',32),'hex'),1,'consent')",
        "UPDATE rec_preference_commands SET created_at=now() WHERE user_id=$1",
    ] {
        let error = sqlx::query(statement)
            .bind(actor)
            .execute(&pool)
            .await
            .expect_err("old binary must fail closed");
        assert_legacy_fence(error);
    }

    // The old consent transaction cannot commit a partial opt-out after its
    // final command write is fenced. Neither old rollback nor DELETE loses P1.
    let mut old_consent = pool.begin().await?;
    sqlx::query("DELETE FROM rec_preference_commands WHERE user_id=$1")
        .bind(actor)
        .execute(&mut *old_consent)
        .await?;
    sqlx::query(
        "UPDATE rec_subjects SET enabled=false,enabled_at=NULL,revision=revision+1,
         subject_id=gen_random_uuid(),feedback_epoch=feedback_epoch+1 WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&mut *old_consent)
    .await?;
    let error = sqlx::query(
        "INSERT INTO rec_preference_commands(
             user_id,command_key,payload_hash,result_revision,command_type)
         VALUES($1,decode(repeat('ab',32),'hex'),decode(repeat('cd',32),'hex'),14,'consent')",
    )
    .bind(actor)
    .execute(&mut *old_consent)
    .await
    .expect_err("old consent writer must roll back");
    assert_legacy_fence(error);
    old_consent.rollback().await?;
    let old_scope: (bool, i64, i64) =
        sqlx::query_as("SELECT enabled,revision,feedback_epoch FROM rec_subjects WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await?;
    assert_eq!(old_scope, (true, 13, 7));
    sqlx::query("DELETE FROM rec_feedback_state")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM rec_preference_commands")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM rec_subjects WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await?;
    assert_eq!(preferences(&pool, actor).await?, expected_preferences);
    assert_eq!(commands(&pool, &actors).await?, expected_commands);

    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(books[0])
        .execute(&pool)
        .await?;
    assert_eq!(preferences(&pool, actor).await?.len(), 2);
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await?;
    assert!(preferences(&pool, actor).await?.is_empty());
    assert!(commands(&pool, &[actor]).await?.is_empty());
    let account_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rec_preference_accounts WHERE user_id=$1)")
            .bind(actor)
            .fetch_one(&pool)
            .await?;
    assert!(!account_exists);

    pool.close().await;
    Ok(())
}
