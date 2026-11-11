//! Installed190 -> primitives191 -> anonymous observation -> atomic retirement.
//! Every installed checksum is compared, with no trigger bypass.
//! Leaves drained synthetic source keys for the separate real HTTP/browser proof.
#[path = "collection_retirement_installed_upgrade_pg/boundaries.rs"]
mod boundaries;
#[path = "collection_retirement_installed_upgrade_pg/budget.rs"]
mod budget;
#[path = "collection_retirement_installed_upgrade_pg/concurrency.rs"]
mod concurrency;
#[path = "collection_retirement_installed_upgrade_pg/contextual.rs"]
mod contextual;
#[path = "collection_retirement_installed_upgrade_pg/fixtures.rs"]
mod fixtures;
#[path = "collection_retirement_installed_upgrade_pg/harness.rs"]
mod harness;
#[path = "collection_retirement_installed_upgrade_pg/proofs.rs"]
mod proofs;
#[path = "collection_retirement_installed_upgrade_pg/ril.rs"]
mod ril;
use fixtures::{count, Fixture, TestResult};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires fresh disposable tor_ci_collection_retirement_upgrade and app role"]
async fn installed_retirement_preserves_business_and_erases_with_bounded_evidence() -> TestResult {
    let owner = harness::connect(
        "COLLECTION_RETIREMENT_UPGRADE_DATABASE_URL",
        "tor_ci_collection_retirement_upgrade",
        "tor_migrator",
        5,
    )
    .await?;
    harness::grant_app(&owner).await?;
    let before = harness::baseline(&owner).await?;
    let fixture = Fixture::seed(&owner).await?;
    ril::seed(&owner, &fixture.ril_batch).await?;
    let canonical = fixture.canonical(&owner).await?;
    let preferences = fixture.preferences(&owner).await?;
    let prepared = harness::through(harness::PRIMITIVES);
    assert_eq!(prepared.migrations.len(), 191);
    prepared.run(&owner).await?;
    assert_eq!(
        count(
            &owner,
            "SELECT count(*) FROM pg_trigger WHERE tgname='retired_collection_write_fence'"
        )
        .await?,
        0,
        "191 is a preparatory test boundary, not a production retirement"
    );
    harness::through(harness::OBSERVATION).run(&owner).await?;
    ril::seed_observed(&owner).await?;
    harness::grant_app(&owner).await?;
    let app = harness::connect(
        "COLLECTION_RETIREMENT_APP_DATABASE_URL",
        "tor_ci_collection_retirement_upgrade",
        "tor_app",
        6,
    )
    .await?;
    // A real old writer holds its table/row lock while the barrier migration
    // waits; it must commit before the database can advertise the new fence.
    let mut old_writer = app.begin().await?;
    sqlx::query("UPDATE rec_assignments SET book_opened_at=now() WHERE id=$1::uuid")
        .bind(&fixture.assignment)
        .execute(&mut *old_writer)
        .await?;
    let migration_pool = owner.clone();
    let migration = tokio::spawn(async move {
        harness::through(harness::BARRIER)
            .run(&migration_pool)
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let waiting = count(
            &owner,
            "SELECT count(*) FROM pg_locks WHERE relation='rec_assignments'::regclass
             AND mode='ShareRowExclusiveLock' AND NOT granted",
        )
        .await?;
        if waiting > 0 {
            break;
        }
        assert!(
            !migration.is_finished(),
            "migration did not wait for the old writer"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "migration lock was not observed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    old_writer.commit().await?;
    migration.await??;
    harness::through(harness::BARRIER).run(&owner).await?;
    harness::assert_ledger(&owner, &before).await?;
    // Re-granting ordinary app DML cannot bypass an installed statement fence.
    harness::grant_app(&owner).await?;
    assert_eq!(fixture.canonical(&owner).await?, canonical);
    assert_eq!(fixture.preferences(&owner).await?, preferences);
    proofs::old_writers_and_canonical_noops(&app, &fixture).await?;
    boundaries::foreign_keys_and_fixed_search_path(&owner).await?;
    boundaries::temp_shadow_cannot_hide_public_erasure(&app, &fixture).await?;
    proofs::scope_cascade_and_integrity(&app, &fixture).await?;
    concurrency::sorted_locks_and_parent_deletes(&app, &fixture).await?;
    ril::mixed_batches(&app, &fixture).await?;
    budget::bounded_rollback_workers_and_retention(&owner, &app, &fixture).await?;
    assert_eq!(fixture.canonical(&owner).await?, canonical);
    assert_eq!(fixture.preferences(&owner).await?, preferences);
    ril::observed_is_preserved(&owner).await?;
    contextual::upgrade(&owner, &app).await?;
    assert_eq!(fixture.canonical(&owner).await?, canonical);
    assert_eq!(fixture.preferences(&owner).await?, preferences);
    ril::observed_is_preserved(&owner).await?;
    app.close().await;
    owner.close().await;
    Ok(())
}
