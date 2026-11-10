//! Positive legacy report regressions use only the explicit 191 fixture database.
use std::process::Command;

pub fn url() -> String {
    let url = std::env::var("LEGACY_ANALYTICS_DATABASE_URL")
        .expect("explicit legacy analytics database required");
    assert!(
        url.starts_with("postgresql://tor_app:")
            && url.ends_with("@127.0.0.1:5432/tor_ci_legacy_analytics")
            && !url.contains('?')
            && !url.contains('#'),
        "legacy report tests require isolated loopback tor_ci_legacy_analytics"
    );
    let output = Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", &url, "--command"])
        .arg(
            "SELECT current_database()='tor_ci_legacy_analytics' AND current_user='tor_app'
             AND (SELECT count(*)=191 AND bool_and(success) AND max(version)=202611163274 FROM _sqlx_migrations)
             AND to_regclass('public.rec_analytics_invalidations') IS NOT NULL
             AND to_regprocedure('public.rec_retire_analytics_batch(integer)') IS NULL
             AND NOT EXISTS(SELECT 1 FROM pg_trigger WHERE tgname='retired_collection_write_fence')",
        )
        .output()
        .expect("legacy PostgreSQL boundary proof");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "t");
    url
}
