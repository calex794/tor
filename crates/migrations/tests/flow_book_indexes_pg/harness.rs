use super::TestResult;
use sqlx::{
    migrate::{MigrateError, Migration, Migrator},
    postgres::{PgConnectOptions, PgPoolOptions},
    Connection, PgConnection, PgPool,
};
use std::{borrow::Cow, env, str::FromStr, time::Duration};
use tor_migrations::repositories::MIGRATOR;

pub const LAST: i64 = 202611294688;
pub const SPECS: [Spec; 2] = [
    Spec {
        version: 202611294687,
        name: "reader_chapter_starts_book_period",
        table: "reader_chapter_starts",
        time: "started_at",
    },
    Spec {
        version: LAST,
        name: "reader_chapter_completions_book_period",
        table: "reader_chapter_completions",
        time: "completed_at",
    },
];

#[derive(Clone, Copy)]
pub struct Spec {
    pub version: i64,
    pub name: &'static str,
    pub table: &'static str,
    pub time: &'static str,
}

impl Spec {
    pub fn migration(self) -> &'static Migration {
        MIGRATOR.iter().find(|m| m.version == self.version).unwrap()
    }

    fn definition(self) -> String {
        format!(
            "CREATE INDEX {} ON public.{} USING btree (book_id, {})",
            self.name, self.table, self.time
        )
    }
}

pub fn through(version: i64) -> Migrator {
    Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|m| m.version <= version)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
}

pub struct Fixture {
    pub pool: PgPool,
    pub options: PgConnectOptions,
}

impl Fixture {
    pub async fn new(database: &str) -> TestResult<Self> {
        assert!([
            "tor_ci_flow_index_fresh",
            "tor_ci_flow_index_upgrade",
            "tor_ci_flow_index_invalid",
            "tor_ci_flow_index_unrecorded",
        ]
        .contains(&database));
        let url = env::var("FLOW_BOOK_INDEX_MIGRATOR_URL")?;
        let options = PgConnectOptions::from_str(&url)?;
        assert_eq!(options.get_host(), "127.0.0.1", "disposable loopback only");
        assert_eq!(options.get_port(), 5432);
        assert_eq!(options.get_database(), Some("tor_ci_flow_index_fresh"));
        assert_eq!(options.get_username(), "tor_migrator");
        let options = options.database(database);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options.clone())
            .await?;
        let actual: (String, String, bool) = sqlx::query_as(
            "SELECT current_database()::text,current_user::text,
             to_regclass('public._sqlx_migrations') IS NULL",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(actual, (database.to_owned(), "tor_migrator".to_owned(), true));
        Ok(Self { pool, options })
    }

    pub async fn migrate(&self, version: i64) -> TestResult<Result<(), MigrateError>> {
        // SQLx keeps a session advisory lock after a failed run. The real
        // operator process exits on failure; close this dedicated fixture
        // connection to model that boundary before the explicit retry.
        let mut connection = PgConnection::connect_with(&self.options).await?;
        sqlx::query("SET statement_timeout='30s'")
            .execute(&mut connection)
            .await?;
        let result = through(version).run(&mut connection).await;
        connection.close().await?;
        Ok(result)
    }
}

pub type Ledger = Vec<(i64, Vec<u8>, bool, String, i64)>;

pub async fn ledger(fixture: &Fixture) -> TestResult<Ledger> {
    Ok(sqlx::query_as(
        "SELECT version,checksum,success,installed_on::text,execution_time
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&fixture.pool)
    .await?)
}

pub async fn assert_ledger(fixture: &Fixture, version: i64) -> TestResult {
    let actual = ledger(fixture).await?;
    let expected = through(version);
    assert_eq!(actual.len(), expected.iter().count());
    for (row, migration) in actual.iter().zip(expected.iter()) {
        assert_eq!(row.0, migration.version);
        assert_eq!(row.1.as_slice(), migration.checksum.as_ref());
        assert!(row.2);
    }
    Ok(())
}

pub async fn index(fixture: &Fixture, spec: Spec, valid: bool) -> TestResult {
    let actual: (bool, bool, String) = sqlx::query_as(
        "SELECT i.indisvalid,i.indisready,pg_get_indexdef(i.indexrelid)
         FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid
         JOIN pg_namespace n ON n.oid=c.relnamespace
         WHERE n.nspname='public' AND c.relname=$1",
    )
    .bind(spec.name)
    .fetch_one(&fixture.pool)
    .await?;
    assert_eq!(actual.0, valid, "{}", spec.name);
    if valid {
        assert!(actual.1);
    }
    assert_eq!(actual.2, spec.definition(), "never repair an unrelated index");
    Ok(())
}

pub async fn repair(fixture: &Fixture, spec: Spec) -> TestResult {
    assert!(SPECS.iter().any(|known| known.name == spec.name));
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=$1)",
    )
    .bind(spec.version)
    .fetch_one(&fixture.pool)
    .await?;
    assert!(!present, "recorded migration is outside this repair recipe");
    // Caller already checked the exact schema/table/columns with index().
    // This is test/operator recovery, never automatic production repair.
    sqlx::query(&format!("DROP INDEX CONCURRENTLY public.{}", spec.name))
        .execute(&fixture.pool)
        .await?;
    let absent: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NULL")
        .bind(format!("public.{}", spec.name))
        .fetch_one(&fixture.pool)
        .await?;
    assert!(absent);
    Ok(())
}

pub async fn seed(fixture: &Fixture) -> TestResult {
    sqlx::raw_sql(include_str!("seed.sql"))
        .execute(&fixture.pool)
        .await?;
    Ok(())
}

pub async fn fingerprint(fixture: &Fixture) -> TestResult<String> {
    Ok(sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'starts',(SELECT jsonb_agg(to_jsonb(s) ORDER BY user_id,book_id,chapter_id)
                    FROM reader_chapter_starts s),
          'completions',(SELECT jsonb_agg(to_jsonb(c) ORDER BY user_id,book_id,chapter_id)
                         FROM reader_chapter_completions c),
          'outbox',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM outbox_events e),
          'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY id) FROM chapter_versions v)
         )::text",
    )
    .fetch_one(&fixture.pool)
    .await?)
}
