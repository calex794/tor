//! Required PostgreSQL schema gate for this binary. A catalog-only readiness
//! check would miss the B-06 private chapter working-pointer table.
pub(super) const REQUIRED_MIGRATION: i64 = 202610260007;

pub(super) fn require_latest_migration(version: Option<i64>) -> Result<(), sqlx::Error> {
    if version == Some(REQUIRED_MIGRATION) {
        Ok(())
    } else {
        Err(sqlx::Error::Protocol(
            "TOR schema missing required B-06 working-drafts migration 202610260007".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{require_latest_migration, REQUIRED_MIGRATION};

    #[test]
    fn old_schema_cannot_report_ready_without_upgrading() {
        assert!(require_latest_migration(None).is_err());
        assert!(require_latest_migration(Some(202610130003)).is_err());
        assert!(require_latest_migration(Some(REQUIRED_MIGRATION)).is_ok());
    }
}
