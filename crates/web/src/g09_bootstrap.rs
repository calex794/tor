//! Explicit operator opt-in and private S3 provider wiring for G-09.
use std::env;
use tor_accounts::repositories::PgAccounts;
use axum::Router;

pub fn enabled(demo: bool) -> Result<bool, &'static str> {
    let enabled = match env::var("TOR_G09_OPERATOR_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => return Err("TOR_G09_OPERATOR_ENABLED must be 0 or 1"),
    };
    if enabled
        && (demo || env::var_os("DATABASE_URL").is_none()
            || env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1"))
    {
        return Err("G-09 requires authenticated PostgreSQL");
    }
    #[cfg(not(feature = "s3-paid"))]
    if enabled {
        return Err("G-09 requires --features s3-paid");
    }
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
            aws_sdk_s3::Client::new(&config), bucket,
        ).map_err(|_| "Invalid G-09 S3 bucket")?;
        Ok(tor_web::g09_operator::router(accounts, store, secure, origin))
    }
    #[cfg(not(feature = "s3-paid"))]
    {
        let _ = (accounts, secure, origin);
        Err("G-09 requires --features s3-paid".into())
    }
}
