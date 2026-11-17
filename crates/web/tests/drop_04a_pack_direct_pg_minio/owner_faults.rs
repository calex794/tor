//! Deterministic owner revocation after a REAL MinIO Range read.
use super::super::pg;
use aws_sdk_s3::Client;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use std::time::Duration;
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::{free_s3_publish::PgFreeS3Publication, PgCatalog};
use tor_contracts::paid_delivery::SecretValue;
use tor_reading::repositories::PgReader;
use tor_storage::{
    domain::provider::StorageError,
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::{
        free_chapter::{FreeChapterObject, FreeChapterStore},
        free_placement::ValidatedPackReference,
    },
};
use tor_web::free_author_publish::PgFreeAuthorPublisher;
use tower::ServiceExt;

#[derive(Clone)]
struct ChangeAfterGet {
    store: S3FreeChapterStore,
    database: String,
    statement: String,
}
#[async_trait::async_trait]
impl FreeChapterStore for ChangeAfterGet {
    async fn put_immutable(
        &self,
        object: &FreeChapterObject,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        self.store.put_immutable(object, bytes).await
    }
    async fn get_verified(&self, object: &FreeChapterObject) -> Result<Vec<u8>, StorageError> {
        self.store.get_verified(object).await
    }
    async fn get_verified_pack(
        &self,
        frame: &ValidatedPackReference,
    ) -> Result<Vec<u8>, StorageError> {
        let bytes = self.store.get_verified_pack(frame).await?;
        pg(&self.database, &self.statement);
        Ok(bytes)
    }
    async fn presign_verified(
        &self,
        object: &FreeChapterObject,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        self.store.presign_verified(object, ttl).await
    }
}

pub(super) async fn verify(
    url: &str,
    client: &Client,
    identity: (i64, i64, i64),
    session: &str,
    original: &str,
) {
    let (actor, book, chapter) = identity;
    let other: i64 = pg(url, "INSERT INTO users(email_normalized,display_name,password_hash) VALUES('drop-owner-revoke@tor.example','Other owner','$argon2id$test') RETURNING id").parse().unwrap();
    let revision: i64 = pg(
        url,
        &format!("SELECT edit_revision FROM chapters WHERE id={chapter}"),
    )
    .parse()
    .unwrap();
    for (change, restore, expected) in [
        (
            format!("UPDATE books SET owner_user_id={other} WHERE id={book}"),
            format!("UPDATE books SET owner_user_id={actor} WHERE id={book}"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("UPDATE chapters SET edit_revision=edit_revision+1 WHERE id={chapter}"),
            format!("UPDATE chapters SET edit_revision={revision} WHERE id={chapter}"),
            StatusCode::CONFLICT,
        ),
        (
            format!("UPDATE books SET admin_hold=true WHERE id={book}"),
            format!("UPDATE books SET admin_hold=false WHERE id={book}"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let catalog = PgCatalog::connect(url).await.unwrap();
        let pool = catalog.pool().clone();
        let publisher = PgFreeAuthorPublisher::new(
            PgFreeS3Publication::new(pool.clone()),
            ChangeAfterGet {
                store: S3FreeChapterStore::new(client.clone(), super::super::BUCKET.into())
                    .unwrap(),
                database: url.into(),
                statement: change,
            },
            super::super::BUCKET.into(),
        )
        .unwrap();
        let app = tor_web::router_with_postgres_reading_auth_free_s3(
            catalog,
            PgReader::new(pool.clone()),
            PgAccounts::from_pool(pool.clone()),
            (false, "http://127.0.0.1:3098".into()),
            None,
            publisher,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/me/books/{book}/chapters/{chapter}/edit"))
                    .header("Cookie", format!("tor-dev-session={session}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            expected,
            "revoked editor must not render verified PACK bytes"
        );
        let body = to_bytes(response.into_body(), 1_048_576).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(original));
        pg(url, &restore);
        pool.close().await;
    }
    println!("DROP04C_OWNER_REVOKE_PASS: real Range then owner/revision/hold change denied");
}
