//! Explicit, audited PACK rollback in a disposable local environment.
#![forbid(unsafe_code)]

#[cfg(feature = "s3-free")]
mod command {
    use aws_sdk_s3::{
        config::{
            retry::RetryConfig, timeout::TimeoutConfig, BehaviorVersion, Credentials, Region,
        },
        Client,
    };
    use reqwest::Url;
    use std::{env, time::Duration};
    use tor_books::repositories::{free_pack_attach::PgFreePackCatalog, PgCatalog};
    use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;
    use tor_web::free_pack_worker::rollback_one_verified_pack;

    const USAGE: &str = "usage: drop_pack_rollback_drill JOB_ID BUILD_EPOCH";

    fn setting(name: &'static str) -> Result<String, &'static str> {
        env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or(name)
    }

    fn local(url: &Url) -> bool {
        url.host_str() == Some("127.0.0.1") && url.query().is_none() && url.fragment().is_none()
    }

    fn store() -> Result<(S3FreeChapterStore, String), &'static str> {
        let endpoint = setting("TOR_DROP_ROLLBACK_S3_ENDPOINT")?;
        let bucket = setting("TOR_DROP_ROLLBACK_S3_BUCKET")?;
        let uri = Url::parse(&endpoint).map_err(|_| "invalid drill S3 endpoint")?;
        if !local(&uri)
            || uri.scheme() != "http"
            || uri.path() != "/"
            || !uri.username().is_empty()
            || uri.password().is_some()
            || !bucket.starts_with("tor-rollback-drill-")
        {
            return Err("rollback drill requires a dedicated loopback S3 bucket");
        }
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(15))
                    .build(),
            )
            .credentials_provider(Credentials::new(
                setting("TOR_DROP_ROLLBACK_S3_ACCESS_KEY")?,
                setting("TOR_DROP_ROLLBACK_S3_SECRET_KEY")?,
                None,
                None,
                "drop-rollback-drill",
            ))
            .build();
        Ok((
            S3FreeChapterStore::new(Client::from_conf(config), bucket.clone())
                .map_err(|_| "invalid drill bucket")?,
            bucket,
        ))
    }

    #[tokio::main]
    pub async fn run() -> Result<(), &'static str> {
        let args: Vec<_> = env::args().skip(1).collect();
        let [job, epoch] = args.as_slice() else {
            return Err(USAGE);
        };
        let job = job.parse::<i64>().map_err(|_| USAGE)?;
        let epoch = epoch.parse::<i64>().map_err(|_| USAGE)?;
        if job <= 0 || epoch <= 0 || setting("TOR_DROP_ROLLBACK_DRILL").as_deref() != Ok("YES") {
            return Err("explicit rollback drill opt-in and positive job and epoch required");
        }
        let database = setting("TOR_DROP_ROLLBACK_DATABASE_URL")?;
        let uri = Url::parse(&database).map_err(|_| "invalid drill PostgreSQL URI")?;
        if !local(&uri)
            || !matches!(uri.scheme(), "postgres" | "postgresql")
            || !matches!(uri.path(), "/tor_ci" | "/tor_drop_rollback_drill")
        {
            return Err("rollback drill requires a disposable loopback database");
        }
        let (store, bucket) = store()?;
        let catalog = PgCatalog::connect(&database)
            .await
            .map_err(|_| "drill PostgreSQL unavailable")?;
        let packs = PgFreePackCatalog::new(catalog.pool().clone());
        let claim = packs
            .load_pack_rollback_job(job, epoch, &bucket)
            .await
            .map_err(|_| "exact succeeded PACK job not available")?;
        let outcome = tokio::time::timeout(
            Duration::from_secs(600),
            rollback_one_verified_pack(&packs, &store, &claim),
        )
        .await
        .map_err(|_| "rollback drill timed out")?
        .map_err(|_| "rollback proof or active generation check failed")?;
        println!("DROP_PACK_ROLLBACK_DRILL_OK job={job} epoch={epoch} result={outcome:?}");
        Ok(())
    }
}

#[cfg(feature = "s3-free")]
fn main() {
    if let Err(message) = command::run() {
        eprintln!("DROP_PACK_ROLLBACK_DRILL_REFUSED {message}");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "s3-free"))]
fn main() {
    eprintln!("DROP_PACK_ROLLBACK_DRILL_REFUSED build with s3-free");
    std::process::exit(2);
}
