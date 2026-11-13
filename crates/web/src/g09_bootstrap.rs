//! Explicit operator opt-in and private S3 provider wiring for G-09.
use axum::Router;
use std::env;
use tor_accounts::repositories::PgAccounts;

#[cfg(feature = "s3-paid")]
mod schedule;

/// No implicit worker. A scheduler requires the separate administrator
/// console opt-in, authenticated PG and an S3-capable build.
fn schedule_opt_in(flag: Option<&str>, operator: bool) -> Result<bool, &'static str> {
    match flag {
        None | Some("0") => Ok(false),
        Some("1") if operator => Ok(true),
        Some("1") => Err("G-09 scheduler requires enabled G-09 operator"),
        _ => Err("TOR_G09_SCHEDULE_ENABLED must be 0 or 1"),
    }
}

fn schedule_setting() -> Result<Option<String>, &'static str> {
    match env::var("TOR_G09_SCHEDULE_ENABLED") {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        _ => Err("Invalid TOR_G09_SCHEDULE_ENABLED"),
    }
}

pub fn enabled(demo: bool) -> Result<bool, &'static str> {
    let enabled = match env::var("TOR_G09_OPERATOR_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => return Err("TOR_G09_OPERATOR_ENABLED must be 0 or 1"),
    };
    if enabled
        && (demo
            || env::var_os("DATABASE_URL").is_none()
            || env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1"))
    {
        return Err("G-09 requires authenticated PostgreSQL");
    }
    #[cfg(not(feature = "s3-paid"))]
    if enabled {
        return Err("G-09 requires --features s3-paid");
    }
    schedule_opt_in(schedule_setting()?.as_deref(), enabled)?;
    Ok(enabled)
}

pub async fn routes(
    accounts: PgAccounts,
    secure: bool,
    origin: String,
) -> Result<Router, Box<dyn std::error::Error>> {
    #[cfg(feature = "s3-paid")]
    {
        let bucket = env::var("TOR_PAID_S3_BUCKET")?;
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let store = tor_storage::repositories::s3::S3PrivateStore::new(
            aws_sdk_s3::Client::new(&config),
            bucket,
        )
        .map_err(|_| "Invalid G-09 S3 bucket")?;
        let run_schedule = schedule_opt_in(schedule_setting()?.as_deref(), true)?;
        if run_schedule {
            // Reject explicit scheduler opt-in without its G-owned migrations.
            // Never change a production schema on HTTP startup.
            tor_storage::repositories::reconciliation_schedule::PgG09Schedule::new(
                accounts.pool().clone(),
            )
            .probe()
            .await
            .map_err(|_| "G-09 scheduler requires existing reconciliation migrations")?;
            let _scheduler = schedule::start(accounts.clone(), store.clone());
            tracing::info!("G-09 read-only LIST/HEAD scheduler enabled; no GET or deletion");
        }
        Ok(tor_web::g09_operator::router(
            accounts, store, secure, origin,
        ))
    }
    #[cfg(not(feature = "s3-paid"))]
    {
        let _ = (accounts, secure, origin);
        Err("G-09 requires --features s3-paid".into())
    }
}

#[cfg(test)]
mod tests {
    use super::schedule_opt_in;

    #[test]
    fn g09_scheduler_fails_closed_without_explicit_operator() {
        assert_eq!(schedule_opt_in(None, false), Ok(false));
        assert_eq!(schedule_opt_in(Some("0"), false), Ok(false));
        assert_eq!(schedule_opt_in(Some("1"), true), Ok(true));
        assert!(schedule_opt_in(Some("1"), false).is_err());
        for invalid in ["", "true", "on", "2", " 1", "yes"] {
            assert!(schedule_opt_in(Some(invalid), true).is_err());
        }
    }
}
