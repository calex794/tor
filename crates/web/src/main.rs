use std::{env, error::Error, net::SocketAddr, sync::Arc};
use tor_accounts::repositories::PgAccounts;
use tor_books::{
    repositories::{InMemoryCatalog, PgCatalog},
    services::PublicBookRepository,
};
use tor_contracts::paid_delivery::PrivateObjectDeliveryPort;
use tor_reading::repositories::PgReader;

/// No schema changes happen during HTTP startup.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind = env::var("TOR_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr = bind.parse()?;
    if !address.ip().is_loopback() {
        return Err("TOR must bind to loopback behind HTTPS reverse proxy".into());
    }
    let demo = env::var("TOR_DEMO_MODE").as_deref() == Ok("1");
    // No paid bearer URL can be issued without an operator-enabled, compiled
    // private G adapter, PostgreSQL and cookie authentication.
    let paid_grants_enabled = match env::var("TOR_PAID_GRANTS_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => return Err("TOR_PAID_GRANTS_ENABLED must be 0 or 1".into()),
    };
    if paid_grants_enabled {
        if demo
            || env::var_os("DATABASE_URL").is_none()
            || env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1")
        {
            return Err("Paid grants require PostgreSQL and enabled cookie authentication".into());
        }
        #[cfg(not(feature = "s3-paid"))]
        return Err("Paid grants require a build with --features s3-paid".into());
    }
    // Optional numeric-only SQLx telemetry. Never enables raw SQL logs.
    if !demo && env::var("DATABASE_URL").is_ok() && !tor_web::activate_ril_sqlx_observation() {
        eprintln!("RIL SQLx completion telemetry disabled (subscriber unavailable)");
    }
    // Private cover S3 is an explicit operator-owned opt-in. A deployment
    // without S3 continues serving books, but never invents image URLs.
    #[cfg(feature = "s3-covers")]
    let cover_store: Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>> =
        match env::var("TOR_COVER_S3_BUCKET") {
            Ok(bucket) => {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                let client = aws_sdk_s3::Client::new(&config);
                let store = tor_ingest::repositories::s3::S3CoverStore::new(client, bucket)?;
                Some(Arc::new(store))
            }
            Err(env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
    #[cfg(not(feature = "s3-covers"))]
    let cover_store: Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>> = {
        if env::var_os("TOR_COVER_S3_BUCKET").is_some() {
            return Err("TOR_COVER_S3_BUCKET requires a build with --features s3-covers".into());
        }
        None
    };
    // Explicit opt-in only. Never start image jobs in demo, without PostgreSQL,
    // without a compiled private S3 adapter, or on malformed configuration.
    let worker_setting = match env::var("TOR_COVER_WORKER_ENABLED") {
        Ok(setting) => Some(setting),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let cover_worker_enabled = match tor_web::cover_worker::worker_opt_in(
        worker_setting.as_deref(),
        demo,
        env::var_os("DATABASE_URL").is_some(),
        cover_store.is_some(),
    ) {
        Ok(enabled) => enabled,
        Err(reason) => return Err(reason.into()),
    };
    // Do not start jobs until the HTTP application and listener have passed
    // their own readiness checks. No second database pool is opened.
    let mut pending_cover_worker: Option<(
        PgCatalog,
        Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
    )> = None;
    let app = match env::var("DATABASE_URL") {
        Ok(url) if !demo => {
            let catalog = PgCatalog::connect(&url).await?;
            // Share one bounded pool across the catalog, reader and accounts,
            // respecting tor_app's six-connection server limit.
            let pool = catalog.pool();
            // Optional RIL telemetry uses the same restricted connection pool.
            // PG outage or unapplied RIL migration never blocks core HTTP routes.
            tor_web::start_ril_pg_background(&catalog);
            let reader = PgReader::new(pool.clone());
            reader.probe().await?;
            if cover_worker_enabled {
                pending_cover_worker = cover_store
                    .as_ref()
                    .map(|storage| (catalog.clone(), storage.clone()));
            }
            println!("TOR PostgreSQL catalogue and public reading on {address}");
            if env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1") {
                let origin = env::var("TOR_PUBLIC_ORIGIN")?;
                let secure = origin.starts_with("https://");
                let uri: axum::http::Uri = origin.parse()?;
                let host = uri.host().unwrap_or("");
                let local = host == "localhost" || host == "127.0.0.1";
                let scheme_ok = uri.scheme_str() == Some("https")
                    || (uri.scheme_str() == Some("http") && local);
                if !scheme_ok
                    || uri.authority().is_none()
                    || uri.authority().is_some_and(|a| a.as_str().contains('@'))
                    || uri.path_and_query().is_some_and(|p| p.as_str() != "/")
                    || origin.ends_with('/')
                {
                    return Err(
                        "TOR_PUBLIC_ORIGIN requires an HTTPS origin (HTTP only for localhost)"
                            .into(),
                    );
                }
                let accounts = PgAccounts::from_pool(pool.clone());
                accounts.probe().await?;
                #[cfg(feature = "s3-paid")]
                let paid_storage: Option<Arc<dyn PrivateObjectDeliveryPort>> =
                    if paid_grants_enabled {
                        let bucket = env::var("TOR_PAID_S3_BUCKET")?;
                        let kek = tor_storage::services::crypto::ContentKek::from_environment()
                            .map_err(|_| "TOR_CONTENT_KEK_V1 is required and must be 32 bytes")?;
                        let config =
                            aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                        let client = aws_sdk_s3::Client::new(&config);
                        let store = tor_storage::repositories::s3::S3PrivateStore::new(
                            client,
                            bucket.clone(),
                        )
                        .map_err(|_| "Invalid TOR_PAID_S3_BUCKET")?;
                        let signer =
                            tor_storage::repositories::paid_delivery::PgPrivateDelivery::new(
                                pool.clone(),
                                store,
                                Arc::new(kek),
                                bucket,
                            )
                            .map_err(|_| "Invalid G paid delivery configuration")?;
                        Some(Arc::new(signer))
                    } else {
                        None
                    };
                #[cfg(not(feature = "s3-paid"))]
                let paid_storage: Option<Arc<dyn PrivateObjectDeliveryPort>> = None;
                if let Some(paid) = paid_storage {
                    tor_web::router_with_postgres_reading_auth_paid_grants(
                        catalog,
                        reader,
                        accounts,
                        secure,
                        origin,
                        paid,
                        cover_store.map(|store| (store, cover_worker_enabled)),
                    )
                } else if let Some(store) = cover_store {
                    if cover_worker_enabled {
                        tor_web::router_with_postgres_reading_auth_cover_uploads(
                            catalog, reader, accounts, secure, origin, store,
                        )
                    } else {
                        tor_web::router_with_postgres_reading_auth_covers(
                            catalog, reader, accounts, secure, origin, store,
                        )
                    }
                } else {
                    tor_web::router_with_postgres_reading_auth(
                        catalog, reader, accounts, secure, origin,
                    )
                }
            } else {
                if let Some(store) = cover_store {
                    tor_web::router_with_postgres_reading_covers(catalog, reader, store)
                } else {
                    tor_web::router_with_postgres_reading(catalog, reader)
                }
            }
        }
        Ok(_) => return Err("Use either DATABASE_URL or TOR_DEMO_MODE=1".into()),
        Err(env::VarError::NotPresent) if demo => {
            let catalog: Arc<dyn PublicBookRepository> = Arc::new(InMemoryCatalog::demo());
            println!("tor S0 demo listening at {address}");
            tor_web::router(catalog)
        }
        Err(env::VarError::NotPresent) => {
            return Err("Set DATABASE_URL for PostgreSQL, or TOR_DEMO_MODE=1 for demo".into());
        }
        Err(error) => return Err(error.into()),
    };
    let listener = tokio::net::TcpListener::bind(address).await?;
    // Exactly one bounded background task for this process. PostgreSQL's
    // SKIP LOCKED and lease_epoch fence competing processes and restarts.
    let _cover_worker = if let Some((catalog, storage)) = pending_cover_worker {
        let started = tor_web::cover_worker::start_cover_worker_background(catalog, storage)
            .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
        println!("TOR private cover worker enabled");
        Some(started)
    } else {
        None
    };
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
