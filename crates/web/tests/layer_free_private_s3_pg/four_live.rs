//! D-05: extend the accepted FREE SINGLE test with two verified paid editions
//! and another FREE version, all in the SAME ordinarily deleted Layer Book.
use super::*;
use std::{fs::{self, OpenOptions}, io::Write, path::Path, sync::Arc};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use tor_books::repositories::paid_storage::{PaidPublication, PgPaidStorage};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId};
use tor_reading::repositories::PgReader;
use tor_storage::{
    repositories::{paid_delivery::PgPrivateDelivery, registry::PgStorageRegistry, s3::S3PrivateStore},
    services::{
        crypto::{seal_chapter, ContentKek},
        provider::upload_and_verify as upload_paid,
    },
};

const FREE_TWO: &str = "Second immutable FREE V1.\n\nNo public fallback.";
const PAID_ONE: &str = "Private historical paid V1 one.\n\nOnly original Layer buyer.";
const PAID_TWO: &str = "Private historical paid V1 two.\n\nNever current V2.";
const KEK: [u8; 32] = [0x77; 32];
const OBJECT_IDS: [&str; 2] = [
    "f0d05005-2026-4005-8005-000000000011",
    "f0d05005-2026-4005-8005-000000000012",
];

pub(super) async fn install(
    url: &str,
    owner: i64,
    book: i64,
    pool: &sqlx::PgPool,
    free: &S3FreeChapterStore,
    client: &Client,
) -> [(i64, i64); 3] {
    let registry = PgStorageRegistry::new(pool.clone());
    let paid_store = S3PrivateStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let kek = Arc::new(ContentKek::from_bytes(KEK));
    let mut editions = Vec::new();
    for (n, text) in [FREE_TWO, PAID_ONE, PAID_TWO].iter().enumerate() {
        let chapter_no = n + 2;
        let access = if n == 0 { "free" } else { "paid" };
        let price = if n == 0 { 0 } else { 50 };
        let chapter: i64 = pg(url, &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},{chapter_no},'D05 additional archive {chapter_no}','public','{access}',{price}) RETURNING id"
        )).parse().unwrap();
        let version = if n == 0 {
            let hash: [u8; 32] = Sha256::digest(text.as_bytes()).into();
            pg(url, &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256) \
                 VALUES({chapter},1,decode('{}','hex')) RETURNING id", hex::encode(hash)
            )).parse::<i64>().unwrap()
        } else {
            pg(url, &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
                 VALUES({chapter},1,'{text}') RETURNING id"
            )).parse::<i64>().unwrap()
        };
        if n == 0 {
            let object = FreeChapterObject::for_markdown(chapter, version, text).unwrap();
            upload_and_verify(free, &object, text).await.unwrap();
            pg(url, &format!(
                "INSERT INTO free_chapter_storage_objects \
                 (chapter_id,version_id,bucket,object_key,sha256,size_bytes) \
                 VALUES({chapter},{version},'{BUCKET}','{}',decode('{}','hex'),{})",
                object.key(), hex::encode(object.sha256()), object.size_bytes()
            ));
            pg(url, &format!(
                "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
            ));
        } else {
            let pinned = ChapterVersionRef { chapter_id: id(chapter), version_id: id(version) };
            let object_id = ObjectId::parse(OBJECT_IDS[n - 1]).unwrap();
            let sealed = seal_chapter(pinned, object_id.clone(), text, &kek).unwrap();
            let reference = registry.reserve_pending(&sealed, &kek, BUCKET).await.unwrap();
            let receipt = upload_paid(&paid_store, &reference, &sealed.ciphertext).await.unwrap();
            registry.mark_ready(&receipt).await.unwrap();
            PgPaidStorage::new(pool.clone()).attach_verified_and_publish(
                owner, book, pinned, &object_id,
                PaidPublication::Publish { expected_current: None },
            ).await.unwrap();
        }
        editions.push((chapter, version));
    }
    editions.try_into().unwrap()
}

pub(super) async fn paid_router(url: &str, accounts: &PgAccounts, client: &Client) -> Router {
    env::set_var("TOR_LAYER_READER_ENABLED", "1");
    let pool = accounts.pool().clone();
    let store = S3PrivateStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let signer = PgPrivateDelivery::new(
        pool.clone(), store, Arc::new(ContentKek::from_bytes(KEK)), BUCKET.to_owned()
    ).unwrap();
    let result = tor_web::router_with_postgres_reading_auth_paid_grants_browser(
        PgCatalog::connect(url).await.unwrap(), PgReader::new(pool), accounts.clone(),
        false, ORIGIN.to_owned(), (Arc::new(signer), ENDPOINT.to_owned()), None,
    ).unwrap();
    env::remove_var("TOR_LAYER_READER_ENABLED");
    result
}
