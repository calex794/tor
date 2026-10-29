//! I-16: four *unchanged* source SQL blobs with provisional unique numbers.
//! This test only observes a disposable tor_test DB AFTER the exact #243->I16
//! SQLx migration upgrade and never applies migrations or writes money.
use sqlx::postgres::PgPoolOptions;
use std::env;
use tor_migrations::repositories::MIGRATOR;

#[tokio::test]
#[ignore = "only run after the exact-base upgrade into disposable tor_test"]
async fn i16_exact_parallel_migrations_applied_and_schema_present() {
    let url = env::var("TOR_UPGRADE_DATABASE_URL")
        .expect("CI must specify disposable tor_test PostgreSQL URL");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect disposable PostgreSQL 18");
    let db: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .expect("identify database");
    assert_eq!(db, "tor_test", "never inspect or modify persistent DBs");

    // Make even accidental future SQL writes fail: the test is observational.
    sqlx::query("SET default_transaction_read_only = on")
        .execute(&pool)
        .await
        .expect("set session read-only");

    // Expected file descriptions are the SQLx underscore-to-space names.
    let required = [
        (202610280014_i64, "layer create idempotency"),
        (202610290001_i64, "reader book follows"),
        (202610300018_i64, "account login events"),
        (202610300019_i64, "referral signup"),
        (202610300020_i64, "direct purchase edition upgrade"),
    ];
    let installed: Vec<(i64, String, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT version, description, checksum, success
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("read installed SQLx entries");
    assert_eq!(installed.len(), MIGRATOR.migrations.len());
    assert_eq!(
        MIGRATOR.migrations.len(),
        52,
        "42 merged-main + 10 Draft-only migrations"
    );
    for (version, description) in required {
        let compiled = MIGRATOR
            .migrations
            .iter()
            .find(|m| m.version == version)
            .unwrap_or_else(|| panic!("missing compiled SQLx migration {version}"));
        assert_eq!(compiled.description, description);
        let applied = installed
            .iter()
            .find(|(v, ..)| *v == version)
            .unwrap_or_else(|| panic!("missing applied SQLx migration {version}"));
        assert_eq!(applied.1, description);
        assert_eq!(applied.2.as_slice(), compiled.checksum.as_ref());
        assert!(applied.3, "SQLx migration {version} failed");
    }

    for (label, sql) in [
        ("A-03 session internal ID",
         "SELECT EXISTS(SELECT 1 FROM information_schema.columns
           WHERE table_schema='public' AND table_name='sessions' AND column_name='session_id')"),
        ("A-03 login event",
         "SELECT to_regclass('public.account_login_events') IS NOT NULL"),
        ("F-09 referral codes",
         "SELECT to_regclass('public.referral_codes') IS NOT NULL"),
        ("F-09 registration outbox",
         "SELECT to_regclass('public.account_registration_referral_outbox') IS NOT NULL"),
        ("D library identity constraint",
         "SELECT EXISTS(SELECT 1 FROM pg_constraint WHERE conname='chapter_purchase_owned_identity_unique')"),
        ("D selected edition trigger",
         "SELECT EXISTS(SELECT 1 FROM pg_trigger WHERE tgname='purchased_library_guard_selected_edition' AND NOT tgisinternal)"),
        ("D selected edition function",
         "SELECT to_regprocedure('public.tor_select_latest_purchased_edition(bigint,bigint,bigint)') IS NOT NULL"),
        ("C-15 book follows",
         "SELECT to_regclass('public.reader_book_follows') IS NOT NULL"),
    ] {
        let exists: bool = sqlx::query_scalar(sql)
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("schema probe {label}: {error}"));
        assert!(exists, "missing integrated schema: {label}");
    }
    pool.close().await;
}
