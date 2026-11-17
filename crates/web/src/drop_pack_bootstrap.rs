//! R4b3: explicit private PACK worker startup, isolated from HTTP routes.
use std::env;
#[cfg(feature = "s3-free")]
use std::error::Error;

pub fn chapter_import_enabled(demo: bool, storage: bool) -> Result<bool, &'static str> {
    let setting = match env::var("TOR_CHAPTER_IMPORT_WORKER_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err("Invalid S023 worker setting"),
    };
    tor_web::chapter_import_worker::worker_opt_in(
        setting.as_deref(),
        demo,
        env::var_os("DATABASE_URL").is_some(),
        env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
        storage,
    )
}

/// DROP-04d: publishing PACK-only chapters requires its reader as well as
/// the existing authenticated, private S023 worker. Default OFF.
pub fn import_enabled(demo: bool, worker: bool) -> Result<bool, &'static str> {
    let setting = match env::var("TOR_CHAPTER_IMPORT_PACK_DIRECT_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err("Invalid PACK-direct import setting"),
    };
    tor_web::chapter_import_worker::direct_pack_opt_in(
        setting.as_deref(),
        demo,
        worker,
        env::var("TOR_FREE_PACK_READER_ENABLED").as_deref() == Ok("1"),
        env::var_os("TOR_FREE_S3_BUCKET").is_some(),
    )
}

#[cfg(feature = "s3-free")]
pub async fn configure_import(
    writer: Option<tor_web::free_author_publish::S3FreeAuthor>,
    enabled: bool,
    worker_enabled: bool,
) -> Result<Option<tor_web::free_author_publish::S3FreeAuthor>, Box<dyn Error>> {
    if worker_enabled {
        writer
            .as_ref()
            .ok_or("S023 import requires FREE S3 publisher")?
            .probe_queued_import_schema()
            .await
            .map_err(|_| "DROP queued import transport migration not applied")?;
    }
    if !enabled && env::var("TOR_FREE_PACK_READER_ENABLED").as_deref() != Ok("1") {
        return Ok(writer);
    }
    if writer.is_none() && !enabled {
        return Ok(None);
    }
    let writer = writer.ok_or("PACK-direct import requires FREE S3 publisher")?;
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    Ok(Some(writer.with_pack_import_policy(
        aws_sdk_s3::Client::new(&config),
        enabled,
    )))
}

#[cfg(feature = "s3-free")]
pub fn start_import(
    catalog: tor_books::repositories::PgCatalog,
    storage: std::sync::Arc<dyn tor_imports::services::ChapterImportObjectStore>,
    writer: Option<tor_web::free_author_publish::S3FreeAuthor>,
) -> tokio::task::JoinHandle<()> {
    match writer {
        Some(writer) => tor_web::chapter_import_worker::start_chapter_import_worker_with_free(
            catalog, storage, writer,
        ),
        None => {
            tor_web::chapter_import_worker::start_chapter_import_worker_background(catalog, storage)
        }
    }
}

pub fn enabled(demo: bool) -> Result<bool, &'static str> {
    let setting = match env::var("TOR_DROP_PACK_WORKER_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err("Invalid DROP worker setting"),
    };
    #[cfg(feature = "s3-free")]
    {
        tor_web::free_pack_worker::runner::worker_opt_in(
            setting.as_deref(),
            demo,
            env::var_os("DATABASE_URL").is_some(),
            env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
            env::var_os("TOR_FREE_S3_BUCKET").is_some(),
        )
    }
    #[cfg(not(feature = "s3-free"))]
    {
        let _ = demo;
        match setting.as_deref() {
            None | Some("0") => Ok(false),
            Some("1") => Err("DROP worker requires --features s3-free"),
            Some(_) => Err("TOR_DROP_PACK_WORKER_ENABLED must be 0 or 1"),
        }
    }
}

#[cfg(feature = "s3-free")]
pub async fn configure(
    catalog: &tor_books::repositories::PgCatalog,
) -> Result<
    tor_web::free_pack_worker::runner::DropPackWorker<
        tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
    >,
    Box<dyn Error>,
> {
    use tor_books::repositories::free_pack_attach::PgFreePackCatalog;
    use tor_storage::repositories::{
        free_chapter_s3::S3FreeChapterStore, free_pack_write_s3::S3FreePackWriter,
    };

    let bucket = env::var("TOR_FREE_S3_BUCKET")?;
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_s3::Client::new(&config);
    let single = S3FreeChapterStore::new(client.clone(), bucket.clone())
        .map_err(|_| "Invalid private DROP FREE S3 bucket")?;
    let b = PgFreePackCatalog::new(catalog.pool().clone());
    b.probe_pack_worker_schema()
        .await
        .map_err(|_| "DROP PACK job schema not migrated")?;
    Ok(tor_web::free_pack_worker::runner::DropPackWorker::new(
        b,
        single,
        S3FreePackWriter::new(client),
        bucket,
        120,
    )?)
}

/// Planning is a separate operator opt-in. Running the R4b3 worker alone
/// never scans, imports or preapproves the catalogue.
pub fn planner_enabled(demo: bool, worker_enabled: bool) -> Result<bool, &'static str> {
    let setting = match env::var("TOR_DROP_PACK_PLANNER_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err("Invalid DROP planner setting"),
    };
    match setting.as_deref() {
        None | Some("0") => Ok(false),
        Some("1") if !demo && worker_enabled => Ok(true),
        Some("1") => Err("DROP planner requires the authenticated private PACK worker"),
        Some(_) => Err("TOR_DROP_PACK_PLANNER_ENABLED must be 0 or 1"),
    }
}

#[cfg(feature = "s3-free")]
pub async fn configure_planner(
    catalog: &tor_books::repositories::PgCatalog,
) -> Result<
    tor_web::free_pack_planner::DropPackPlanner<
        tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
    >,
    Box<dyn Error>,
> {
    use tor_books::repositories::{
        free_pack_attach::PgFreePackCatalog, free_pack_planner::PackPlannerPolicy,
    };
    use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;

    let bucket = env::var("TOR_FREE_S3_BUCKET")?;
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_s3::Client::new(&config);
    let single = S3FreeChapterStore::new(client, bucket.clone())
        .map_err(|_| "Invalid DROP planner FREE S3 bucket")?;
    let b = PgFreePackCatalog::new(catalog.pool().clone());
    b.probe_pack_planner_schema()
        .await
        .map_err(|_| "DROP pack planner migration not applied")?;
    Ok(tor_web::free_pack_planner::DropPackPlanner::new(
        b,
        single,
        bucket,
        PackPlannerPolicy::default(),
    )?)
}

/// DROP-05b: only an independently opted-in authenticated runtime may
/// start the queued active-placement cutover. Separate from the planner,
/// R4b3 candidate builder and PACK-direct import. Default OFF.
pub fn cutover_enabled(demo: bool, worker_enabled: bool) -> Result<bool, &'static str> {
    let setting = match env::var("TOR_DROP_PACK_CUTOVER_ENABLED") {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(_) => return Err("Invalid DROP cutover setting"),
    };
    #[cfg(feature = "s3-free")]
    {
        tor_web::free_pack_worker::activation::cutover_opt_in(
            setting.as_deref(),
            demo,
            env::var_os("DATABASE_URL").is_some(),
            env::var("TOR_AUTH_ENABLED").as_deref() == Ok("1"),
            env::var_os("TOR_FREE_S3_BUCKET").is_some(),
            env::var("TOR_FREE_PACK_READER_ENABLED").as_deref() == Ok("1"),
            worker_enabled,
        )
    }
    #[cfg(not(feature = "s3-free"))]
    {
        let _ = (demo, worker_enabled);
        match setting.as_deref() {
            None | Some("0") => Ok(false),
            Some("1") => Err("DROP cutover requires --features s3-free"),
            Some(_) => Err("TOR_DROP_PACK_CUTOVER_ENABLED must be 0 or 1"),
        }
    }
}

/// Only when separately opted in. Do not apply database migrations at startup.
#[cfg(feature = "s3-free")]
pub async fn configure_cutover(
    catalog: &tor_books::repositories::PgCatalog,
) -> Result<
    tor_web::free_pack_worker::activation::DropPackActivationWorker<
        tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
    >,
    Box<dyn Error>,
> {
    use tor_books::repositories::free_pack_attach::PgFreePackCatalog;
    use tor_storage::repositories::{
        free_chapter_s3::S3FreeChapterStore, free_pack_proof_s3::S3PackProofStore,
    };

    let bucket = env::var("TOR_FREE_S3_BUCKET")?;
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_s3::Client::new(&config);
    let single = S3FreeChapterStore::new(client.clone(), bucket.clone())
        .map_err(|_| "Invalid private DROP activation bucket")?;
    let b = PgFreePackCatalog::new(catalog.pool().clone());
    b.probe_pack_worker_schema()
        .await
        .map_err(|_| "DROP PACK worker schema not migrated")?;
    b.probe_pack_activation_schema()
        .await
        .map_err(|_| "DROP PACK activation schema not migrated")?;
    Ok(
        tor_web::free_pack_worker::activation::DropPackActivationWorker::new(
            b,
            single,
            S3PackProofStore::new(client),
            bucket,
            120,
        )?,
    )
}

/// Configure all optional DROP background services at the same authenticated
/// PG readiness boundary. The independent cutover flag remains default OFF.
#[cfg(feature = "s3-free")]
pub struct PreparedDropServices {
    worker: Option<
        tor_web::free_pack_worker::runner::DropPackWorker<
            tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
        >,
    >,
    planner: Option<
        tor_web::free_pack_planner::DropPackPlanner<
            tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
        >,
    >,
    cutover: Option<
        tor_web::free_pack_worker::activation::DropPackActivationWorker<
            tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
        >,
    >,
}

#[cfg(feature = "s3-free")]
pub async fn configure_services(
    catalog: &tor_books::repositories::PgCatalog,
    worker_enabled: bool,
    planner_enabled: bool,
    cutover_enabled: bool,
) -> Result<PreparedDropServices, Box<dyn Error>> {
    Ok(PreparedDropServices {
        worker: if worker_enabled {
            Some(configure(catalog).await?)
        } else {
            None
        },
        planner: if planner_enabled {
            Some(configure_planner(catalog).await?)
        } else {
            None
        },
        cutover: if cutover_enabled {
            Some(configure_cutover(catalog).await?)
        } else {
            None
        },
    })
}

#[cfg(feature = "s3-free")]
pub fn start_services(
    services: PreparedDropServices,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    (
        services
            .worker
            .map(tor_web::free_pack_worker::runner::start_background),
        services
            .planner
            .map(tor_web::free_pack_planner::start_background),
        services
            .cutover
            .map(tor_web::free_pack_worker::activation::start_background),
    )
}
