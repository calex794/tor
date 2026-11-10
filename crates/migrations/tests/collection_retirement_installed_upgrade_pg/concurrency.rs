use super::{
    fixtures::{count, Fixture, TestResult},
    proofs::code,
};
use sqlx::PgPool;
use std::time::Duration;

async fn wait_for_lock(pool: &PgPool, name: &str) -> TestResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
             WHERE application_name=$1 AND wait_event_type='Lock')",
        )
        .bind(name)
        .fetch_one(pool)
        .await?;
        if waiting {
            return Ok(());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected lock wait: {name}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn busy(pool: &PgPool) -> TestResult {
    let result: (i64, bool) = sqlx::query_as("SELECT * FROM rec_retire_analytics_batch(7)")
        .fetch_one(pool)
        .await?;
    assert_eq!(
        result,
        (0, true),
        "another eraser is progress pending, not completion or failure"
    );
    Ok(())
}

pub async fn sorted_locks_and_parent_deletes(app: &PgPool, f: &Fixture) -> TestResult {
    let canonical = f.canonical(app).await?;
    let preferences = f.preferences(app).await?;
    let mut lower = app.begin().await?;
    sqlx::query("SELECT 1 FROM flow_visit_daily WHERE landing_surface=1 FOR UPDATE")
        .fetch_all(&mut *lower)
        .await?;
    let pool = app.clone();
    let subject = f.subject.clone();
    let deleter =
        tokio::spawn(async move {
            let mut tx = pool.begin().await?;
            sqlx::raw_sql(
            "SET LOCAL application_name='ci_retirement_sorted'; SET LOCAL statement_timeout='5s'",
        ).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM public.flow_visits WHERE subject_id=$1::uuid")
                .bind(subject)
                .execute(&mut *tx)
                .await?;
            tx.rollback().await
        });
    wait_for_lock(app, "ci_retirement_sorted").await?;
    let mut upper = app.begin().await?;
    sqlx::query("SELECT 1 FROM flow_visit_daily WHERE landing_surface=2 FOR UPDATE NOWAIT")
        .fetch_all(&mut *upper)
        .await?;
    upper.rollback().await?;
    lower.rollback().await?;
    deleter.await??;

    // A legacy explicit scope call uses the same gate before either daily
    // table. The next batch must not take leaves while that transaction is live.
    let mut old = app.begin().await?;
    sqlx::query("SELECT flow_erase_scope($1::uuid,$2::uuid)")
        .bind(&f.subject)
        .bind(&f.epoch)
        .execute(&mut *old)
        .await?;
    busy(app).await?;
    old.rollback().await?;

    // All three real parent paths acquire the statement gate before row locks:
    // user -> scope cascade, book -> aggregate/raw cascade, outbox -> C receipt.
    let parents = [
        ("users", "id", f.cascade_actor.to_string()),
        ("books", "id", f.cascade_book.to_string()),
        ("outbox_events", "id", format!("'{}'::uuid", f.delete_event)),
    ];
    for (table, key, value) in parents {
        let mut parent = app.begin().await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE {key}={value}"))
            .execute(&mut *parent)
            .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *parent)
            .await?;
        busy(app).await?;
        parent.rollback().await?;

        // Reverse interleaving: the batch owns the gate first; the parent
        // must wait before locking its row. A third connection proves that.
        let mut batch = app.begin().await?;
        let result: (i64, bool) = sqlx::query_as("SELECT * FROM rec_retire_analytics_batch(1)")
            .fetch_one(&mut *batch)
            .await?;
        assert!(result.0 <= 1 && result.1);
        let pool = app.clone();
        let delete = format!("DELETE FROM {table} WHERE {key}={value}");
        let name = format!("ci_retirement_parent_{table}");
        let task_name = name.clone();
        let parent = tokio::spawn(async move {
            let mut tx = pool.begin().await?;
            sqlx::query("SELECT set_config('application_name',$1,true)")
                .bind(task_name)
                .execute(&mut *tx)
                .await?;
            sqlx::query("SET LOCAL statement_timeout='5s'")
                .execute(&mut *tx)
                .await?;
            sqlx::query(&delete).execute(&mut *tx).await?;
            tx.rollback().await
        });
        wait_for_lock(app, &name).await?;
        let mut probe = app.begin().await?;
        sqlx::query(&format!(
            "SELECT 1 FROM {table} WHERE {key}={value} FOR UPDATE NOWAIT"
        ))
        .fetch_all(&mut *probe)
        .await?;
        probe.rollback().await?;
        batch.rollback().await?;
        parent.await??;
    }
    assert_eq!(f.canonical(app).await?, canonical);
    assert_eq!(f.preferences(app).await?, preferences);

    // A raw batch timeout is a transaction rollback, not a partial zero. A
    // caller may already hold arbitrary locks, so timeouts/retry remain needed.
    let before = count(app, "SELECT count(*) FROM rec_dirty_buckets").await?;
    let mut blocker = app.begin().await?;
    sqlx::query("SELECT 1 FROM flow_book_daily ORDER BY cohort_day,book_id FOR UPDATE")
        .fetch_all(&mut *blocker)
        .await?;
    let mut batch = app.begin().await?;
    sqlx::query("SET LOCAL statement_timeout='2s'")
        .execute(&mut *batch)
        .await?;
    let error = sqlx::query("SELECT * FROM rec_retire_analytics_batch(1000)")
        .execute(&mut *batch)
        .await
        .unwrap_err();
    code(error, "55P03");
    batch.rollback().await?;
    blocker.rollback().await?;
    assert_eq!(
        count(app, "SELECT count(*) FROM rec_dirty_buckets").await?,
        before
    );
    assert_eq!(f.canonical(app).await?, canonical);
    assert_eq!(f.preferences(app).await?, preferences);
    Ok(())
}
