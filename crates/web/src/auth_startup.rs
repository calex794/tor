//! Startup composition for authenticated PG, paid and S3-only free chapters.
//! Author content is never persisted here. No startup S3 or PG migrations.
use std::{env, error::Error, sync::Arc};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_contracts::paid_delivery::PrivateObjectDeliveryPort;
use tor_reading::repositories::PgReader;

type Cover = Arc<dyn tor_ingest::services::cover_jobs::CoverObjectStore>;

pub struct AuthInputs {
    pub catalog: PgCatalog,
    pub reader: PgReader,
    pub accounts: PgAccounts,
    pub secure: bool,
    pub origin: String,
    pub covers: Option<Cover>,
    pub cover_worker_enabled: bool,
    pub paid_storage: Option<Arc<dyn PrivateObjectDeliveryPort>>,
    #[cfg(feature = "s3-paid")]
    pub paid_author: Option<
        tor_web::PgPaidAuthorPublisher<tor_storage::repositories::s3::S3PrivateStore>,
    >,
    #[cfg(feature = "s3-free")]
    pub free_author: Option<
        tor_web::free_author_publish::PgFreeAuthorPublisher<
            tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
        >,
    >,
}

impl AuthInputs {
    pub fn router(self) -> Result<axum::Router, Box<dyn Error>> {
        let catalog = self.catalog;
        let reader = self.reader;
        let accounts = self.accounts;
        let secure = self.secure;
        let origin = self.origin;
        let covers = self.covers.map(|store| (store, self.cover_worker_enabled));
        if let Some(paid) = self.paid_storage {
            let s3_origin = env::var("TOR_PAID_S3_PUBLIC_ORIGIN")?;
            #[cfg(all(feature = "s3-paid", feature = "s3-free"))]
            {
                let result = match (self.paid_author, self.free_author) {
                    (Some(author), Some(free)) => {
                        tor_web::router_with_postgres_reading_auth_paid_grants_browser_author_free(
                            catalog, reader, accounts, (secure, origin),
                            (paid, s3_origin), covers, author, free,
                        )
                    }
                    (Some(author), None) => {
                        tor_web::router_with_postgres_reading_auth_paid_grants_browser_author(
                            catalog, reader, accounts, (secure, origin),
                            (paid, s3_origin), covers, author,
                        )
                    }
                    (None, Some(free)) => {
                        tor_web::router_with_postgres_reading_auth_paid_grants_browser_free_author(
                            catalog, reader, accounts, (secure, origin),
                            (paid, s3_origin), covers, free,
                        )
                    }
                    (None, None) => {
                        tor_web::router_with_postgres_reading_auth_paid_grants_browser(
                            catalog, reader, accounts, secure, origin,
                            (paid, s3_origin), covers,
                        )
                    }
                };
                return result.map_err(|reason| reason.into());
            }
            #[cfg(all(feature = "s3-paid", not(feature = "s3-free")))]
            {
                let result = if let Some(author) = self.paid_author {
                    tor_web::router_with_postgres_reading_auth_paid_grants_browser_author(
                        catalog, reader, accounts, (secure, origin),
                        (paid, s3_origin), covers, author,
                    )
                } else {
                    tor_web::router_with_postgres_reading_auth_paid_grants_browser(
                        catalog, reader, accounts, secure, origin, (paid, s3_origin), covers,
                    )
                };
                return result.map_err(|reason| reason.into());
            }
            #[cfg(not(feature = "s3-paid"))]
            {
                return tor_web::router_with_postgres_reading_auth_paid_grants_browser(
                    catalog, reader, accounts, secure, origin, (paid, s3_origin), covers,
                )
                .map_err(|reason| reason.into());
            }
        }
        #[cfg(feature = "s3-free")]
        if let Some(free) = self.free_author {
            return Ok(tor_web::router_with_postgres_reading_auth_free_s3(
                catalog, reader, accounts, (secure, origin), covers, free,
            ));
        }
        if let Some((store, enabled)) = covers {
            return Ok(if enabled {
                tor_web::router_with_postgres_reading_auth_cover_uploads(
                    catalog, reader, accounts, secure, origin, store,
                )
            } else {
                tor_web::router_with_postgres_reading_auth_covers(
                    catalog, reader, accounts, secure, origin, store,
                )
            });
        }
        Ok(tor_web::router_with_postgres_reading_auth(
            catalog, reader, accounts, secure, origin,
        ))
    }
}

#[cfg(feature = "s3-free")]
pub async fn configured_free_publisher(
    catalog: &PgCatalog,
) -> Result<
    Option<
        tor_web::free_author_publish::PgFreeAuthorPublisher<
            tor_storage::repositories::free_chapter_s3::S3FreeChapterStore,
        >,
    >,
    Box<dyn Error>,
> {
    let bucket = match env::var("TOR_FREE_S3_BUCKET") {
        Ok(bucket) => bucket,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_s3::Client::new(&config);
    let store = tor_storage::repositories::free_chapter_s3::S3FreeChapterStore::new(
        client,
        bucket.clone(),
    )
    .map_err(|_| "Invalid TOR_FREE_S3_BUCKET")?;
    let publisher = tor_web::free_author_publish::PgFreeAuthorPublisher::new(
        tor_books::repositories::free_s3_publish::PgFreeS3Publication::new(catalog.pool()),
        store,
        bucket,
    )?;
    Ok(Some(publisher))
}
