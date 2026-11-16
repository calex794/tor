//! R4b3: explicit private PACK worker startup, isolated from HTTP routes.
use std::env;
#[cfg(feature = "s3-free")]
use std::error::Error;

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
