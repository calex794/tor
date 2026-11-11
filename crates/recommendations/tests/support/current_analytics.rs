//! Ordinary settings and catalogue proofs require the installed retirement fence.
use sqlx::{postgres::PgConnectOptions, PgPool};
use std::str::FromStr;

pub fn url(variable: &str, role: &str) -> String {
    let url = std::env::var(variable).expect("explicit current-schema fixture URL");
    let options = PgConnectOptions::from_str(&url).expect("valid fixture connection options");
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_port(), 5432);
    assert_eq!(options.get_database(), Some("tor_ci"));
    assert_eq!(options.get_username(), role);
    url
}

pub async fn assert_schema(pool: &PgPool) {
    let (database, count, last, successful, barrier): (String, i64, i64, bool, bool) =
        sqlx::query_as(
            "SELECT current_database()::text,count(*),max(version),bool_and(success),
               to_regclass('public.rec_analytics_invalidations') IS NOT NULL
               AND to_regprocedure('public.rec_retire_analytics_batch(integer)') IS NOT NULL
               AND to_regprocedure('public.rec_contextual_policy_guard()') IS NOT NULL
               AND EXISTS(SELECT 1 FROM pg_trigger
                 WHERE tgrelid='public.rec_subjects'::regclass
                   AND tgname='retired_collection_write_fence')
             FROM public._sqlx_migrations",
        )
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(database, "tor_ci");
    assert_eq!((count, last, successful), (194, 202611198437, true));
    assert!(
        barrier,
        "ordinary business must work behind the installed retirement fence"
    );
}
