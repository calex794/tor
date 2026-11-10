//! Retained collectors run only against the explicitly installed 191 baseline.
use sqlx::{postgres::PgConnectOptions, PgPool};
use std::str::FromStr;

pub fn url(variable: &str, role: &str) -> String {
    let url = std::env::var(variable).expect("explicit legacy analytics fixture URL");
    let options = PgConnectOptions::from_str(&url).expect("valid fixture connection options");
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_port(), 5432);
    assert_eq!(options.get_database(), Some("tor_ci_legacy_analytics"));
    assert_eq!(options.get_username(), role);
    url
}

pub async fn assert_schema(pool: &PgPool) {
    let (database, count, last, successful, barrier): (String, i64, i64, bool, bool) =
        sqlx::query_as(
            "SELECT current_database()::text,count(*),max(version),
               bool_and(success) AND to_regclass('public.rec_analytics_invalidations') IS NOT NULL,
               to_regprocedure('public.rec_retire_analytics_batch(integer)') IS NOT NULL
               OR EXISTS(SELECT 1 FROM pg_trigger
                 WHERE tgname='retired_collection_write_fence')
             FROM public._sqlx_migrations",
        )
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci_legacy_analytics");
    assert_eq!((count, last, successful), (191, 202611163274, true));
    assert!(
        !barrier,
        "never bypass the current retirement fence for a positive collector test"
    );
}
