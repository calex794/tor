//! Real 193 -> 194 cutover: legacy rows remain labelled and cannot be reactivated.
use super::{
    fixtures::{run, TestResult},
    harness,
};
use sqlx::PgPool;
use tor_migrations::repositories::MIGRATOR;

async fn rejected(pool: &PgPool, sql: &str) {
    let error = sqlx::query(sql)
        .execute(pool)
        .await
        .expect_err("legacy or forged model accepted");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
}
pub async fn upgrade(owner: &PgPool, app: &PgPool) -> TestResult {
    let before: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(owner)
            .await?;
    assert_eq!(before.len(), 193);
    let old:String=sqlx::query_scalar("INSERT INTO rec_generations(status,finished_at,manifest_hash) VALUES('validated',now(),decode(repeat('ab',32),'hex')) RETURNING id::text").fetch_one(app).await?;
    let building: String =
        sqlx::query_scalar("INSERT INTO rec_generations DEFAULT VALUES RETURNING id::text")
            .fetch_one(app)
            .await?;
    sqlx::query("UPDATE rec_runtime_policy SET active_generation=$1::uuid,previous_generation=$1::uuid,rollout_percent=25,hold=false WHERE singleton").bind(&old).execute(app).await?;
    let rev: i64 = sqlx::query_scalar("SELECT revision FROM rec_runtime_policy WHERE singleton")
        .fetch_one(app)
        .await?;
    MIGRATOR.run(owner).await?;
    MIGRATOR.run(owner).await?;
    harness::grant_app(owner).await?;
    let after: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(owner)
            .await?;
    assert_eq!(after.len(), 194);
    assert_eq!(&after[..193], before.as_slice());
    assert_eq!(after[193].0, 202611198437);
    assert!(after[193].2);
    let policy:(bool,bool,bool,i16,i64)=sqlx::query_as("SELECT active_generation IS NULL,previous_generation IS NULL,hold,rollout_percent,revision FROM rec_runtime_policy WHERE singleton").fetch_one(app).await?;
    assert_eq!(policy, (true, true, true, 0, rev + 1));
    let state: (String, String) =
        sqlx::query_as("SELECT model_version,status FROM rec_generations WHERE id=$1::uuid")
            .bind(&building)
            .fetch_one(app)
            .await?;
    assert_eq!(state, ("legacy_feedback_v1".into(), "rejected".into()));
    rejected(app, "INSERT INTO rec_generations DEFAULT VALUES").await;
    rejected(
        app,
        &format!("UPDATE rec_generations SET model_version='metadata_manual_v1' WHERE id='{old}'"),
    )
    .await;
    rejected(
        app,
        &format!("UPDATE rec_runtime_policy SET active_generation='{old}' WHERE singleton"),
    )
    .await;
    rejected(
        app,
        &format!("UPDATE rec_runtime_policy SET previous_generation='{old}' WHERE singleton"),
    )
    .await;
    rejected(
        app,
        "UPDATE rec_runtime_policy SET rollout_percent=25 WHERE singleton",
    )
    .await;
    let new:String=sqlx::query_scalar("INSERT INTO rec_generations(model_version) VALUES('metadata_manual_v1') RETURNING id::text").fetch_one(app).await?;
    rejected(
        app,
        &format!("UPDATE rec_runtime_policy SET active_generation='{new}' WHERE singleton"),
    )
    .await;
    rejected(
        app,
        &format!("UPDATE rec_generations SET status='validated' WHERE id='{new}'"),
    )
    .await;
    rejected(
        app,
        &format!("UPDATE rec_generations SET model_version='legacy_feedback_v1' WHERE id='{new}'"),
    )
    .await;
    run(app,&format!("UPDATE rec_generations SET status='validated',finished_at=now(),manifest_hash=decode(repeat('cd',32),'hex') WHERE id='{new}'")).await?;
    run(app,&format!("UPDATE rec_runtime_policy SET active_generation='{new}',rollout_percent=100,hold=false WHERE singleton")).await?;
    rejected(
        app,
        &format!("UPDATE rec_generations SET cursor_book_id=1 WHERE id='{new}'"),
    )
    .await;
    // No serving side-effect survives into the following HTTP/browser fixtures.
    run(app,"UPDATE rec_runtime_policy SET active_generation=NULL,previous_generation=NULL,rollout_percent=0,hold=true WHERE singleton").await?;
    run(
        app,
        &format!("DELETE FROM rec_generations WHERE id IN ('{old}','{building}','{new}')"),
    )
    .await?;
    Ok(())
}
