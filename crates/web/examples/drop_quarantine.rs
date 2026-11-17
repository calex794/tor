//! Manual, bounded FREE-key quarantine and isolated retirement exercise.
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
    use tor_books::repositories::{free_key_quarantine::PgFreeKeyQuarantine, PgCatalog};
    use tor_storage::repositories::free_quarantine_s3::S3FreeQuarantineStore;
    use tor_web::free_pack_worker::quarantine_operator::{
        enter_quarantine, recheck_quarantine, retire_drill_object, VersionLocator,
    };

    const USAGE: &str =
        "Usage: drop_quarantine enter|recheck|retire-drill KEY VERSION [BACKUP_VERSION]";

    fn setting(name: &'static str) -> Result<String, &'static str> {
        env::var(name).ok().filter(|s| !s.is_empty()).ok_or(name)
    }

    fn loopback(url: &Url) -> bool {
        matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
    }

    fn object_store(
        archive: bool,
        disposable: bool,
    ) -> Result<(String, S3FreeQuarantineStore), &'static str> {
        let (locator, account, secret, region, bucket) = if archive {
            (
                "TOR_DROP_ARCHIVE_ENDPOINT",
                "TOR_DROP_ARCHIVE_ACCESS_KEY",
                "TOR_DROP_ARCHIVE_SECRET_KEY",
                "TOR_DROP_ARCHIVE_REGION",
                "TOR_DROP_ARCHIVE_BUCKET",
            )
        } else {
            (
                "TOR_DROP_PRIMARY_ENDPOINT",
                "TOR_DROP_PRIMARY_ACCESS_KEY",
                "TOR_DROP_PRIMARY_SECRET_KEY",
                "TOR_DROP_PRIMARY_REGION",
                "TOR_DROP_PRIMARY_BUCKET",
            )
        };
        let endpoint = setting(locator)?;
        let uri = Url::parse(&endpoint).map_err(|_| "invalid object store endpoint")?;
        if uri.host_str().is_none()
            || (!uri.username().is_empty() || uri.password().is_some())
            || uri.query().is_some()
            || uri.fragment().is_some()
            || uri.path() != "/"
            || (uri.scheme() != "https" && !(uri.scheme() == "http" && loopback(&uri)))
            || (disposable && uri.host_str() != Some("127.0.0.1"))
        {
            return Err("object store endpoint is out of scope");
        }
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(setting(region)?))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(20))
                    .build(),
            )
            .credentials_provider(Credentials::new(
                setting(account)?,
                setting(secret)?,
                None,
                None,
                "drop-quarantine",
            ))
            .build();
        Ok((
            setting(bucket)?,
            S3FreeQuarantineStore::new(Client::from_conf(config)),
        ))
    }

    #[tokio::main]
    pub async fn run() -> Result<(), &'static str> {
        let args: Vec<_> = env::args().skip(1).collect();
        let (mode, key, version, archive_version) = match args.as_slice() {
            [mode, key, version] if mode == "enter" || mode == "retire-drill" => {
                (mode.as_str(), key, version, None)
            }
            [mode, key, version, copy] if mode == "recheck" => {
                (mode.as_str(), key, version, Some(copy))
            }
            _ => return Err(USAGE),
        };
        let disposable = mode == "retire-drill";
        let database = setting("TOR_DROP_AUDIT_DATABASE_URL")?;
        if disposable {
            let uri = Url::parse(&database).map_err(|_| "invalid drill PostgreSQL URI")?;
            if !loopback(&uri)
                || !matches!(uri.path(), "/tor_ci" | "/tor_drop_gc_drill")
                || !matches!(uri.scheme(), "postgresql" | "postgres")
                || uri.query().is_some()
                || uri.fragment().is_some()
            {
                return Err("retirement requires an isolated loopback database");
            }
        }
        let catalog = PgCatalog::connect(&database)
            .await
            .map_err(|_| "PostgreSQL unavailable")?;
        let db = PgFreeKeyQuarantine::new(catalog.pool().clone());
        let (bucket, primary) = object_store(false, disposable)?;
        let locator = VersionLocator {
            bucket: &bucket,
            key,
            version,
        };
        let result = match mode {
            "enter" => enter_quarantine(&db, &primary, &locator).await,
            "recheck" => {
                let (archive_bucket, archive) = object_store(true, false)?;
                let backup = VersionLocator {
                    bucket: &archive_bucket,
                    key,
                    version: archive_version.expect("validated arguments"),
                };
                recheck_quarantine(&db, &primary, &archive, &locator, &backup).await
            }
            "retire-drill" => {
                let (archive_bucket, archive) = object_store(true, true)?;
                let release = db
                    .find_release(&bucket, key, version)
                    .await
                    .map_err(|_| "retirement evidence could not be loaded")?
                    .ok_or("retirement evidence has not been recorded")?;
                if archive_bucket != release.backup_bucket {
                    return Err("archive bucket does not match approved evidence");
                }
                retire_drill_object(&db, &primary, &archive, &locator).await
            }
            _ => return Err(USAGE),
        };
        match result {
            Ok(outcome) => {
                println!("DROP_QUARANTINE_OK {outcome:?}");
                Ok(())
            }
            Err(error) => {
                eprintln!("DROP_QUARANTINE_REFUSED {error:?}");
                Err("exact evidence could not be accepted")
            }
        }
    }
}

#[cfg(feature = "s3-free")]
fn main() {
    if let Err(message) = command::run() {
        eprintln!("{message}");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "s3-free"))]
fn main() {
    eprintln!("DROP_QUARANTINE_REFUSED: build with s3-free");
    std::process::exit(2);
}
