//! Real S065 private image fixture: a verified MinIO object bound to the exact
//! unreleased V1 before B publishes the chapter. Only disposable CI services.
use super::*;
use tor_books::repositories::paid_assets::PgPaidAssets;
use tor_storage::services::crypto::{seal_asset_with_keyring, ContentKeyring};

pub(super) const ASSET_OBJECT_ID: &str = "f0c0e034-2026-4034-8034-000000000036";

pub(super) async fn upload_exact_v1_image(
    catalog: &PgCatalog,
    s3: &S3PrivateStore,
    kek: Arc<ContentKek>,
    author: i64,
    pinned: ChapterVersionRef,
) {
    // One valid opaque 1x1 RGBA PNG; its bytes exist only encrypted in MinIO.
    const PNG_HEX: &str = concat!(
        "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154",
        "789c63f8cfc0f01f00050001ff89993d1d0000000049454e44ae426082"
    );
    let pool = catalog.pool();
    let png = hex::decode(PNG_HEX).expect("fixed one-pixel CI PNG");
    let ring = Arc::new(ContentKeyring::v1_only(kek));
    let id = ObjectId::parse(ASSET_OBJECT_ID).unwrap();
    let sealed = seal_asset_with_keyring(pinned, id.clone(), &png, "image/png", &ring).unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let reference = registry
        .reserve_pending_asset(&sealed, &ring, BUCKET)
        .await
        .unwrap();
    let receipt = upload_and_verify(s3, &reference, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt).await.unwrap();
    PgPaidAssets::new(pool.clone())
        .attach_prepared(author, pinned, 1, "image/png", &id)
        .await
        .unwrap();
}
