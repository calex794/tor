//! I/C FLOW-05: real SQLx concurrent indexes and fail-closed interrupted upgrade.
//! Requires four fresh disposable databases; never edits an existing ledger.
#[path = "flow_book_indexes_pg/harness.rs"]
mod harness;
#[path = "flow_book_indexes_pg/interrupted.rs"]
mod interrupted;

use harness::{
    assert_ledger, fingerprint, index, ledger, repair, seed, Fixture, Spec, LAST, SPECS,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn both_index_migrations_keep_sqlx_no_transaction_metadata() {
    for spec in SPECS {
        assert!(spec.migration().no_tx);
        assert!(spec.migration().sql.starts_with("-- no-transaction\n"));
    }
}

#[tokio::test]
#[ignore = "requires fresh disposable tor_ci_flow_index_* databases owned by tor_migrator"]
async fn fresh_sqlx_schema_and_replay_keep_valid_book_period_indexes() -> TestResult {
    let fixture = Fixture::new("tor_ci_flow_index_fresh").await?;
    fixture.migrate(LAST).await??;
    seed(&fixture).await?;
    assert_ledger(&fixture, LAST).await?;
    for spec in SPECS {
        index(&fixture, spec, true).await?;
    }
    let before = ledger(&fixture).await?;
    let facts = fingerprint(&fixture).await?;
    fixture.migrate(LAST).await??;
    assert_eq!(ledger(&fixture).await?, before);
    assert_eq!(fingerprint(&fixture).await?, facts);
    Ok(())
}

#[tokio::test]
#[ignore = "requires fresh disposable tor_ci_flow_index_* databases owned by tor_migrator"]
async fn installed_ledger_upgrade_preserves_canonical_facts_and_old_checksums() -> TestResult {
    let fixture = Fixture::new("tor_ci_flow_index_upgrade").await?;
    fixture.migrate(SPECS[0].version - 1).await??;
    seed(&fixture).await?;
    let before = ledger(&fixture).await?;
    let facts = fingerprint(&fixture).await?;
    fixture.migrate(LAST).await??;
    let after = ledger(&fixture).await?;
    assert_eq!(after.len(), before.len() + 2);
    assert_eq!(&after[..before.len()], before.as_slice());
    assert_ledger(&fixture, LAST).await?;
    for spec in SPECS {
        index(&fixture, spec, true).await?;
    }
    assert_eq!(fingerprint(&fixture).await?, facts);
    fixture.migrate(LAST).await??;
    assert_eq!(ledger(&fixture).await?, after);
    assert_eq!(fingerprint(&fixture).await?, facts);
    Ok(())
}

#[tokio::test]
#[ignore = "requires fresh disposable tor_ci_flow_index_* databases owned by tor_migrator"]
async fn cancelled_build_leaves_invalid_index_and_retry_requires_explicit_repair() -> TestResult {
    let fixture = Fixture::new("tor_ci_flow_index_invalid").await?;
    fixture.migrate(SPECS[0].version - 1).await??;
    seed(&fixture).await?;
    let facts = fingerprint(&fixture).await?;
    for spec in SPECS {
        let before = ledger(&fixture).await?;
        interrupted::cancel_during_writer_wait(&fixture, spec).await?;
        index(&fixture, spec, false).await?;
        assert_eq!(ledger(&fixture).await?, before);
        assert_duplicate_rejected(&fixture, spec).await?;
        assert_eq!(ledger(&fixture).await?, before);
        repair(&fixture, spec).await?;
        assert_eq!(ledger(&fixture).await?, before, "repair never edits ledger");
        fixture.migrate(spec.version).await??;
        index(&fixture, spec, true).await?;
        assert_ledger(&fixture, spec.version).await?;
        assert_eq!(fingerprint(&fixture).await?, facts);
    }
    let accepted = ledger(&fixture).await?;
    fixture.migrate(LAST).await??;
    assert_eq!(ledger(&fixture).await?, accepted);
    for spec in SPECS {
        index(&fixture, spec, true).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires fresh disposable tor_ci_flow_index_* databases owned by tor_migrator"]
async fn committed_index_without_ledger_is_not_silently_accepted_on_retry() -> TestResult {
    use sqlx::{Connection, Executor, PgConnection};

    let fixture = Fixture::new("tor_ci_flow_index_unrecorded").await?;
    fixture.migrate(SPECS[0].version - 1).await??;
    seed(&fixture).await?;
    let facts = fingerprint(&fixture).await?;
    for spec in SPECS {
        let before = ledger(&fixture).await?;
        // Exact DDL, deliberately without SQLx's following ledger INSERT.
        // This reproduces the durable state after losing that commit boundary.
        let mut connection = PgConnection::connect_with(&fixture.options).await?;
        connection.execute(spec.migration().sql.as_ref()).await?;
        connection.close().await?;
        index(&fixture, spec, true).await?;
        assert_eq!(ledger(&fixture).await?, before);
        assert_duplicate_rejected(&fixture, spec).await?;
        assert_eq!(ledger(&fixture).await?, before);
        repair(&fixture, spec).await?;
        assert_eq!(ledger(&fixture).await?, before);
        fixture.migrate(spec.version).await??;
        index(&fixture, spec, true).await?;
        assert_ledger(&fixture, spec.version).await?;
        assert_eq!(fingerprint(&fixture).await?, facts);
    }
    let accepted = ledger(&fixture).await?;
    fixture.migrate(LAST).await??;
    assert_eq!(ledger(&fixture).await?, accepted);
    for spec in SPECS {
        index(&fixture, spec, true).await?;
    }
    Ok(())
}

async fn assert_duplicate_rejected(fixture: &Fixture, spec: Spec) -> TestResult {
    let error = fixture
        .migrate(spec.version)
        .await?
        .expect_err("an existing index without its ledger entry must fail closed");
    let sqlx::migrate::MigrateError::ExecuteMigration(error, version) = error else {
        panic!("unexpected migration error: {error:?}");
    };
    assert_eq!(version, spec.version);
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42P07"),
        "a valid or INVALID duplicate relation must never become migration success"
    );
    Ok(())
}
