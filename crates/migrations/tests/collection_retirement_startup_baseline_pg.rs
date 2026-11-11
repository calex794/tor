//! Prepare a separate actual-binary database with real pre-barrier fixtures.
//! Unlike the installed-upgrade proof, this target deliberately does not drain.
#[allow(dead_code)] // Shared fixtures expose extra assertions to the installed-upgrade target.
#[path = "collection_retirement_installed_upgrade_pg/fixtures.rs"]
mod fixtures;
#[path = "collection_retirement_installed_upgrade_pg/harness.rs"]
mod harness;
#[allow(dead_code)] // This target seeds the old mixed batch; the upgrade target replays it.
#[path = "collection_retirement_installed_upgrade_pg/ril.rs"]
mod ril;
use fixtures::{count, Fixture, TestResult};

#[tokio::test]
#[ignore = "requires fresh disposable tor_ci_collection_retirement_startup owned by tor_migrator"]
async fn prepare_real_installed_retirement_state_for_production_startup() -> TestResult {
    let owner = harness::connect(
        "COLLECTION_RETIREMENT_STARTUP_MIGRATOR_URL",
        "tor_ci_collection_retirement_startup",
        "tor_migrator",
        2,
    )
    .await?;
    harness::grant_app(&owner).await?;
    let before = harness::baseline(&owner).await?;
    let fixture = Fixture::seed(&owner).await?;
    let referenced_generation: String = sqlx::query_scalar(
        "INSERT INTO rec_generations(status,finished_at,manifest_hash) VALUES('validated',now(),decode(repeat('ef',32),'hex')) RETURNING id::text"
    ).fetch_one(&owner).await?;
    sqlx::query("UPDATE rec_assignments SET generation_id=$1::uuid WHERE id=$2::uuid")
        .bind(&referenced_generation)
        .bind(&fixture.assignment)
        .execute(&owner)
        .await?;
    ril::seed(&owner, &fixture.ril_batch).await?;
    let canonical = fixture.canonical(&owner).await?;
    let preferences = fixture.preferences(&owner).await?;
    harness::through(harness::BARRIER).run(&owner).await?;
    harness::through(harness::BARRIER).run(&owner).await?;
    harness::assert_ledger(&owner, &before).await?;
    assert_eq!(fixture.canonical(&owner).await?, canonical);
    assert_eq!(fixture.preferences(&owner).await?, preferences);
    assert_eq!(count(&owner, "SELECT count(*) FROM rec_subjects").await?, 2);
    assert_eq!(count(&owner, "SELECT count(*) FROM flow_visits").await?, 6);
    assert_eq!(
        count(&owner, "SELECT count(*) FROM flow_book_cohorts").await?,
        2
    );
    assert_eq!(
        count(&owner, "SELECT count(*) FROM rec_analytics_invalidations").await?,
        0
    );
    tor_migrations::repositories::MIGRATOR.run(&owner).await?;
    tor_migrations::repositories::MIGRATOR.run(&owner).await?;
    assert_eq!(
        count(
            &owner,
            "SELECT count(*) FROM _sqlx_migrations WHERE success"
        )
        .await?,
        194
    );
    assert_eq!(fixture.canonical(&owner).await?, canonical);
    assert_eq!(fixture.preferences(&owner).await?, preferences);
    assert_eq!(count(&owner, "SELECT count(*) FROM rec_subjects").await?, 2);
    // The new FK never mutates a fenced historical assignment to delete its model.
    let kept: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rec_assignments WHERE id=$1::uuid AND generation_id=$2::uuid)",
    )
    .bind(&fixture.assignment)
    .bind(&referenced_generation)
    .fetch_one(&owner)
    .await?;
    assert!(kept);
    let delete = sqlx::query("DELETE FROM rec_generations WHERE id=$1::uuid")
        .bind(&referenced_generation)
        .execute(&owner)
        .await
        .expect_err("referenced historical model deleted");
    let restriction = delete.as_database_error().expect("database restriction");
    assert_eq!(restriction.code().as_deref(), Some("23001"));
    assert_eq!(
        restriction.constraint(),
        Some("rec_assignments_generation_id_fkey")
    );
    let retained_assignment: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rec_assignments WHERE id=$1::uuid AND generation_id=$2::uuid)",
    )
    .bind(&fixture.assignment)
    .bind(&referenced_generation)
    .fetch_one(&owner)
    .await?;
    assert!(
        retained_assignment,
        "restriction changed the old assignment"
    );
    assert_eq!(count(&owner, "SELECT count(*) FROM rec_subjects").await?, 2);
    owner.close().await;
    Ok(())
}
