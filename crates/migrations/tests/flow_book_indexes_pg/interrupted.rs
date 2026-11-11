use super::{
    harness::{through, Fixture, Spec},
    TestResult,
};
use sqlx::{Connection, PgConnection};
use std::time::Duration;
use tokio::time::{sleep, timeout, Instant};

pub async fn cancel_during_writer_wait(fixture: &Fixture, spec: Spec) -> TestResult {
    // A real old writer blocks the first concurrent validation phase, after
    // the invalid index has already committed. No pg_catalog writes or sleeps
    // chosen to guess when index creation happens.
    let mut writer = fixture.pool.begin().await?;
    sqlx::query(&format!(
        "LOCK TABLE public.{} IN ROW EXCLUSIVE MODE",
        spec.table
    ))
    .execute(&mut *writer)
    .await?;
    let mut connection = PgConnection::connect_with(&fixture.options).await?;
    sqlx::query("SET statement_timeout='15s'")
        .execute(&mut connection)
        .await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut connection)
        .await?;
    let build = tokio::spawn(async move {
        let result = through(spec.version).run(&mut connection).await;
        (result, connection)
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(
               SELECT 1 FROM pg_stat_progress_create_index
               WHERE pid=$1 AND phase='waiting for writers before build'
             ) AND to_regclass($2) IS NOT NULL",
        )
        .bind(pid)
        .bind(format!("public.{}", spec.name))
        .fetch_one(&fixture.pool)
        .await?;
        if waiting {
            break;
        }
        assert!(Instant::now() < deadline, "concurrent build never reached its writer wait");
        sleep(Duration::from_millis(20)).await;
    }

    // The concurrent build still permits another writer's table lock.
    let mut next_writer = fixture.pool.begin().await?;
    sqlx::query(&format!(
        "LOCK TABLE public.{} IN ROW EXCLUSIVE MODE NOWAIT",
        spec.table
    ))
    .execute(&mut *next_writer)
    .await?;
    next_writer.rollback().await?;
    let cancelled: bool = sqlx::query_scalar("SELECT pg_cancel_backend($1)")
        .bind(pid)
        .fetch_one(&fixture.pool)
        .await?;
    assert!(cancelled);
    let (result, connection) = timeout(Duration::from_secs(5), build).await??;
    connection.close().await?;
    writer.rollback().await?;
    let error = result.expect_err("cancelled concurrent build unexpectedly committed");
    let sqlx::migrate::MigrateError::ExecuteMigration(error, version) = error else {
        panic!("unexpected cancellation error: {error:?}");
    };
    assert_eq!(version, spec.version);
    assert_eq!(
        error.as_database_error().and_then(|error| error.code()).as_deref(),
        Some("57014")
    );
    Ok(())
}
