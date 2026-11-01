//! Local MinIO + disposable tor_ci G/B fixture, not application code.
use super::*;
use tor_books::repositories::paid_assets::PgPaidAssets;
use tor_storage::services::crypto::{seal_asset_with_keyring, ContentKeyring};

pub(super) async fn attach_verified_illustration(
    author: i64,
    pinned: ChapterVersionRef,
    pool: &sqlx::PgPool,
    registry: &PgStorageRegistry,
    s3: &S3PrivateStore,
    kek: &Arc<ContentKek>,
) -> ObjectId {
    // A private illustration is G-verified and B-attached before publication.
    // It shares the same immutable V1 as the encrypted paid chapter.
    let image_id = ObjectId::parse("f0c0e034-2026-4034-8034-000000000036").unwrap();
    let ring = ContentKeyring::v1_only(kek.clone());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0; 48]);
    let sealed =
        seal_asset_with_keyring(pinned, image_id.clone(), &png, "image/png", &ring).unwrap();
    let reference = registry
        .reserve_pending_asset(&sealed, &ring, BUCKET)
        .await
        .unwrap();
    let receipt = upload_and_verify(s3, &reference, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt).await.unwrap();
    PgPaidAssets::new(pool.clone())
        .attach_prepared(author, pinned, 1, "image/png", &image_id)
        .await
        .unwrap();
    image_id
}
