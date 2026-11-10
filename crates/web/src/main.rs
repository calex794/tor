mod book_schedule_worker;
mod g09_bootstrap;
mod moderation_overdue_bell_worker;
mod moderation_overdue_worker;
mod social_notification_consumer;
mod support_cleanup;
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
    // Creating paid ciphertext is a separate explicit operator opt-in:
    // enabling signed reads must never silently enable an author S3 writer.
    let paid_author_enabled = match env::var("TOR_PAID_AUTHOR_PUBLISH_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => return Err("TOR_PAID_AUTHOR_PUBLISH_ENABLED must be 0 or 1".into()),
    };
    if paid_author_enabled && !paid_grants_enabled {
        return Err("Paid author publication requires enabled paid grants".into());
    }
    let g09_operator_enabled = g09_bootstrap::enabled(demo)?;
    #[cfg(not(feature = "s3-paid"))]
    if paid_author_enabled {
        return Err("Paid author publication requires a build with --features s3-paid".into());
    }
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
    // CDN public bucket is a separate explicit opt-in. This only configures
    // the adapter: it does not publish, delete, purge, or start any worker.
    let cdn_config = tor_web::cover_cdn_config::from_environment(
        demo,
        env::var_os("DATABASE_URL").is_some(),
        cfg!(feature = "s3-covers"),
    )?;
    #[cfg(not(feature = "s3-covers"))]
    let _ = &cdn_config;
    // Private cover S3 is an explicit operator-owned opt-in. A deployment
    // without S3 continues serving books, but never invents image URLs.
    #[cfg(feature = "s3-covers")]
    let cover_store: Option<Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>> =
        match env::var("TOR_COVER_S3_BUCKET") {
            Ok(bucket) => {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                let client = aws_sdk_s3::Client::new(&config);
                let store = tor_ingest::repositories::s3::S3CoverStore::new(client, bucket)?;
                let store = if let Some(cdn) = cdn_config.as_ref() {
                    store.with_public_cdn(cdn.bucket.clone(), cdn.origin.clone())?
                } else {
                    store
                };
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
    // Chapter-import staging is a separate private S3 capability. It stores
    // only the author's already-reviewed canonical UTF-8 text.
    #[cfg(feature = "s3-chapter-import")]
    let chapter_import_store: Option<Arc<dyn tor_imports::services::ChapterImportObjectStore>> =
        match env::var("TOR_CHAPTER_IMPORT_S3_BUCKET") {
            Ok(bucket) => {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                let client = aws_sdk_s3::Client::new(&config);
                let store =
                    tor_imports::repositories::s3::S3ChapterImportStore::new(client, bucket)
                        .map_err(|_| "Invalid TOR_CHAPTER_IMPORT_S3_BUCKET")?;
                Some(Arc::new(store))
            }
            Err(env::VarError::NotPresent) => None,
            Err(error) => return Err(error.into()),
        };
    #[cfg(not(feature = "s3-chapter-import"))]
    let chapter_import_store: Option<Arc<dyn tor_imports::services::ChapterImportObjectStore>> = {
        if env::var_os("TOR_CHAPTER_IMPORT_S3_BUCKET").is_some() {
            return Err(
                "TOR_CHAPTER_IMPORT_S3_BUCKET requires a build with --features s3-chapter-import"
                    .into(),
            );
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
    // Separate explicit operator approval. Configuring the CDN bucket/HTML
    // does not publish covers or start an external Yandex purge worker.
    let cdn_worker_setting = match env::var("TOR_CDN_COVER_LIFECYCLE_WORKER_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let cdn_worker_enabled = tor_web::cover_cdn_lifecycle::worker_opt_in(
        cdn_worker_setting.as_deref(),
        demo,
        env::var_os("DATABASE_URL").is_some(),
        env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
        cover_store.is_some(),
        cdn_config.is_some(),
    )
    .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
    // Validate the resource and bearer before binding HTTP. Never print secrets
    // or redirect the management client to a user/environment-supplied host.
    let cdn_credentials = if cdn_worker_enabled {
        let resource = env::var("TOR_CDN_COVER_YANDEX_RESOURCE_ID")
            .map_err(|_| "missing S025 CDN resource ID")?;
        let bearer =
            env::var("TOR_CDN_COVER_YANDEX_BEARER").map_err(|_| "missing S025 CDN bearer")?;
        Some((resource, bearer))
    } else {
        None
    };
    let chapter_import_worker_setting = match env::var("TOR_CHAPTER_IMPORT_WORKER_ENABLED") {
        Ok(setting) => Some(setting),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let chapter_import_worker_enabled = match tor_web::chapter_import_worker::worker_opt_in(
        chapter_import_worker_setting.as_deref(),
        demo,
        env::var_os("DATABASE_URL").is_some(),
        env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
        chapter_import_store.is_some(),
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
    let mut pending_cdn_lifecycle: Option<(
        PgCatalog,
        Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>,
    )> = None;
    // These are scoped across the runtime branches and only populated after
    // PostgreSQL, authenticated HTTP and the import-object store are available.
    let mut pending_chapter_import_worker: Option<(
        PgCatalog,
        Arc<dyn tor_imports::services::ChapterImportObjectStore>,
    )> = None;
    let mut pending_chapter_import_routes: Option<axum::Router> = None;
    let mut pending_g09_routes: Option<axum::Router> = None;
    let mut pending_ranking_consumer: Option<tor_recommendations::repositories::PgRanking> = None;
    let mut pending_achievements_consumer: Option<
        tor_recommendations::repositories::PgAchievements,
    > = None;
    let mut pending_referral_consumer: Option<tor_referrals::repositories::PgReferrals> = None;
    // The product switch still controls the catalogue recommendation surface.
    // Retired collection flags cannot enable identity capture or block AUTH=0.
    if let Ok(value) = env::var("TOR_RECOMMENDATIONS_ENABLED") {
        if !matches!(value.as_str(), "0" | "1") {
            return Err("invalid boolean flag TOR_RECOMMENDATIONS_ENABLED".into());
        }
    }
    let mut pending_social_notifications = None;
    let mut pending_support_cleanup = None;
    let mut pending_security_retention: Option<PgAccounts> = None;
    // Explicit operator opt-in. A copied binary must never silently run
    // scheduled releases before the additive S058 migration is applied.
    let schedule_worker_enabled = match env::var("TOR_SCHEDULED_RELEASE_WORKER_ENABLED") {
        Ok(value) if value == "1" => true,
        Ok(value) if value == "0" => false,
        Err(env::VarError::NotPresent) => false,
        _ => return Err("TOR_SCHEDULED_RELEASE_WORKER_ENABLED must be 0 or 1".into()),
    };
    if schedule_worker_enabled
        && (demo
            || env::var_os("DATABASE_URL").is_none()
            || env::var("TOR_AUTH_ENABLED").as_deref() != Ok("1"))
    {
        return Err("S058 worker requires authenticated PostgreSQL runtime".into());
    }
    // Disabled unless explicitly enabled by an operator. This only scans and
    // stores overdue observations; no notification delivery is activated.
    let overdue_setting = match env::var("TOR_MOD_OVERDUE_OBSERVER_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let overdue_observer_enabled = moderation_overdue_worker::worker_opt_in(
        overdue_setting.as_deref(),
        demo,
        env::var_os("DATABASE_URL").is_some(),
        env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
    )
    .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
    // The bell is a distinct explicit opt-in: enabling observation alone
    // must never start recipient-facing delivery.
    let bell_setting =
        moderation_overdue_bell_worker::read_setting("TOR_MOD_OVERDUE_BELL_DELIVERY_ENABLED")?;
    let bell_recipients =
        moderation_overdue_bell_worker::read_setting("TOR_MOD_OVERDUE_BELL_RECIPIENT_POLICY")?;
    let bell_thresholds =
        moderation_overdue_bell_worker::read_setting("TOR_MOD_OVERDUE_BELL_SLA_POLICY")?;
    let bell_delivery_enabled = moderation_overdue_bell_worker::worker_opt_in(
        bell_setting.as_deref(),
        bell_recipients.as_deref(),
        bell_thresholds.as_deref(),
        overdue_observer_enabled,
        demo,
        env::var_os("DATABASE_URL").is_some(),
        env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
    )
    .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
    let mut pending_overdue_bell: Option<tor_social::repositories::notifications::PgNotifications> =
        None;
    let mut pending_overdue_observer: Option<PgAccounts> = None;
    let mut pending_scheduled_release_worker = None;
    let app = match env::var("DATABASE_URL") {
        Ok(url) if !demo => {
            let catalog = PgCatalog::connect(&url).await?;
            // Share one bounded pool across the catalog, reader and accounts,
            // respecting tor_app's six-connection server limit.
            let pool = catalog.pool();
            pending_security_retention = Some(PgAccounts::from_pool(pool.clone()));
            if env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1") {
                pending_support_cleanup = Some(
                    tor_support::repositories::postgres::PgSupport::new(pool.clone()),
                );
                pending_ranking_consumer = Some(tor_recommendations::repositories::PgRanking::new(
                    pool.clone(),
                ));
                pending_achievements_consumer = Some(
                    tor_recommendations::repositories::PgAchievements::new(pool.clone()),
                );
                pending_referral_consumer =
                    Some(tor_referrals::repositories::PgReferrals::new(pool.clone()));
                pending_social_notifications = Some(
                    tor_social::repositories::notifications::PgNotifications::new(pool.clone()),
                );
            }
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
            if cdn_worker_enabled {
                pending_cdn_lifecycle = cover_store
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
                if g09_operator_enabled {
                    pending_g09_routes = Some(
                        g09_bootstrap::routes(accounts.clone(), secure, origin.clone()).await?,
                    );
                }
                if overdue_observer_enabled {
                    accounts
                        .probe_moderation_overdue_observations()
                        .await
                        .map_err(|_| "MOD overdue observation ledger migration is not applied")?;
                    pending_overdue_observer = Some(accounts.clone());
                }
                if bell_delivery_enabled {
                    // This path is allowed only after the live authenticated
                    // ledger probe; no new pool or migration is created.
                    pending_overdue_bell = Some(
                        tor_social::repositories::notifications::PgNotifications::new(pool.clone()),
                    );
                }
                if schedule_worker_enabled {
                    let author = tor_books::repositories::postgres::PgAuthoring::new(pool.clone());
                    author
                        .probe_scheduled_releases()
                        .await
                        .map_err(|_| "S058 scheduled release migration is not applied")?;
                    author
                        .probe_book_schedule_worker()
                        .await
                        .map_err(|_| "S058 book-level slot migrations are not applied")?;
                    pending_scheduled_release_worker = Some(author);
                }
                if chapter_import_worker_enabled {
                    tor_imports::repositories::postgres::PgImportJobs::new(pool.clone())
                        .probe()
                        .await
                        .map_err(|_| "S023 chapter import migration is not applied")?;
                    let storage = chapter_import_store
                        .as_ref()
                        .expect("validated chapter import S3 configuration")
                        .clone();
                    pending_chapter_import_worker = Some((catalog.clone(), storage.clone()));
                    pending_chapter_import_routes =
                        Some(tor_web::chapter_import_queue_http::router(
                            accounts.clone(),
                            secure,
                            origin.clone(),
                            storage,
                        ));
                }
                #[cfg(feature = "s3-paid")]
                let (paid_storage, paid_author): (
                    Option<Arc<dyn PrivateObjectDeliveryPort>>,
                    Option<
                        tor_web::PgPaidAuthorPublisher<
                            tor_storage::repositories::s3::S3PrivateStore,
                        >,
                    >,
                ) = if paid_grants_enabled {
                    let bucket = env::var("TOR_PAID_S3_BUCKET")?;
                    let keyring = Arc::new(
                        tor_storage::services::crypto::ContentKeyring::from_environment()
                            .map_err(|_| "Invalid private content KEK V1/V2 configuration")?,
                    );
                    let config =
                        aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                    let client = aws_sdk_s3::Client::new(&config);
                    let store =
                        tor_storage::repositories::s3::S3PrivateStore::new(client, bucket.clone())
                            .map_err(|_| "Invalid TOR_PAID_S3_BUCKET")?;
                    let author = if paid_author_enabled {
                        Some(
                            tor_web::PgPaidAuthorPublisher::new(
                                catalog.clone(),
                                store.clone(),
                                keyring.clone(),
                                bucket.clone(),
                            )
                            .map_err(|_| "Invalid paid author publisher configuration")?,
                        )
                    } else {
                        None
                    };
                    let signer =
                        tor_storage::repositories::paid_delivery::PgPrivateDelivery::with_keyring(
                            pool.clone(),
                            store,
                            keyring,
                            bucket,
                        )
                        .map_err(|_| "Invalid G paid delivery configuration")?;
                    (Some(Arc::new(signer)), author)
                } else {
                    (None, None)
                };
                #[cfg(not(feature = "s3-paid"))]
                let paid_storage: Option<Arc<dyn PrivateObjectDeliveryPort>> = None;
                if let Some(paid) = paid_storage {
                    // The browser receives only this server-owned allowed S3
                    // origin. No signed host or bucket comes from a visitor.
                    let s3_origin = env::var("TOR_PAID_S3_PUBLIC_ORIGIN")?;
                    #[cfg(feature = "s3-paid")]
                    let app = if let Some(author) = paid_author {
                        tor_web::router_with_postgres_reading_auth_paid_grants_browser_author(
                            catalog,
                            reader,
                            accounts,
                            (secure, origin),
                            (paid, s3_origin),
                            cover_store.map(|store| (store, cover_worker_enabled)),
                            author,
                        )
                    } else {
                        tor_web::router_with_postgres_reading_auth_paid_grants_browser(
                            catalog,
                            reader,
                            accounts,
                            secure,
                            origin,
                            (paid, s3_origin),
                            cover_store.map(|store| (store, cover_worker_enabled)),
                        )
                    };
                    #[cfg(not(feature = "s3-paid"))]
                    let app = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
                        catalog,
                        reader,
                        accounts,
                        secure,
                        origin,
                        (paid, s3_origin),
                        cover_store.map(|store| (store, cover_worker_enabled)),
                    );
                    app.map_err(|reason| -> Box<dyn Error> { reason.into() })?
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
    let app = [pending_g09_routes, pending_chapter_import_routes]
        .into_iter()
        .flatten()
        .fold(app, |app, routes| {
            // The base router already has RIL instrumentation. Instrument each
            // late-bound route group with the same process recorder before merge.
            app.merge(tor_web::instrument_additional_routes(routes))
        });
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
    let _cdn_lifecycle_worker = if let Some((catalog, storage)) = pending_cdn_lifecycle {
        let (resource, bearer) = cdn_credentials.expect("validated opt-in credentials");
        let worker = tor_web::cover_cdn_lifecycle::start(catalog, storage, resource, bearer)
            .map_err(|_| "invalid S025 CDN provider configuration")?;
        println!("TOR opt-in S025 approved-cover CDN lifecycle worker enabled");
        Some(worker)
    } else {
        None
    };
    let _overdue_observer = pending_overdue_observer.map(|accounts| {
        println!("TOR moderation overdue observation enabled");
        moderation_overdue_worker::start(accounts)
    });
    let _overdue_bell_worker = pending_overdue_bell.map(|notifications| {
        println!("TOR moderation overdue in-app admin bell enabled (no external delivery)");
        moderation_overdue_bell_worker::start(notifications)
    });
    let _scheduled_release_worker = pending_scheduled_release_worker.map(|author| {
        println!("TOR S058 scheduled release worker enabled");
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                for _ in 0..16 {
                    match author.process_one_due_scheduled_release().await {
                        Ok(Some((_chapter, _outcome))) => (),
                        Ok(None) => break,
                        Err(error) => {
                            tracing::warn!(?error, "S058 worker failed; lease retry pending");
                            break;
                        }
                    }
                }
                if let Err(error) = book_schedule_worker::drain_due_book_slots(&author).await {
                    tracing::warn!(?error, "S058 book-slot worker failed; lease retry pending");
                }
            }
        })
    });
    let _chapter_import_worker = pending_chapter_import_worker.map(|(catalog, storage)| {
        println!("TOR chapter import worker enabled");
        tor_web::chapter_import_worker::start_chapter_import_worker_background(catalog, storage)
    });
    // C completion events remain durable if the server restarts.
    // Only the authenticated PostgreSQL runtime consumes them.
    let _ranking_consumer =
        pending_ranking_consumer.map(tor_web::ranking_consumer::start_rankings_consumer);
    let _achievements_consumer = pending_achievements_consumer
        .map(tor_web::achievement_consumer::start_achievements_consumer);
    // Registrations and their outbox facts were committed atomically.
    // Replay pending facts after restart; only the authenticated PG runtime runs this.
    let _referral_consumer =
        pending_referral_consumer.map(tor_web::referral_consumer::start_referral_consumer);
    let _social_notification_consumer =
        pending_social_notifications.map(social_notification_consumer::start);
    let _support_cleanup = pending_support_cleanup.map(support_cleanup::start);
    // Every PostgreSQL server retains cleanup, including public AUTH=0 mode.
    let account_retention =
        tor_web::start_account_security_retention(pending_security_retention).await;
    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await;
    if let Some(handle) = account_retention {
        handle.abort();
        let _ = handle.await;
    }
    serve_result?;
    Ok(())
}
