//! H/UI-20: real read-only PostgreSQL eligibility for purchase alternatives.
//! The synthetic object is metadata in disposable tor_ci, never delivered as paid text.
use super::*;
use super::layer_paid_sales::paid;
use tor_commerce::repositories::{
    layer_package_sales::PackagePaidMode,
    layer_storefront::{exclusive_package_for_chapter, exclusive_package_link},
};

async fn attach_test_ciphertext(pool: &PgPool, chapter: i64, version: i64) {
    let object_id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO storage_objects
           (id,object_kind,bucket,object_key,state,size_bytes,
            sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
            kek_id,object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES (
           $1::UUID,'chapter','tor-ci-private',
           'private/chapter-versions/' || $1::UUID::text || '.enc',
           'attached',42,
           decode(repeat('11',32),'hex'),
           sha256(convert_to('H19 private','UTF8')),
           decode(repeat('22',48),'hex'),
           decode(repeat('33',12),'hex'),
           'content-kek-v1',
           decode(repeat('44',12),'hex'),
           convert_to(
             'tor.encrypted.chapter.v1' || chr(10) ||
             $2::BIGINT::text || chr(10) ||
             $3::BIGINT::text || chr(10) ||
             $1::UUID::text || chr(10) ||
             encode(sha256(convert_to('H19 private','UTF8')),'hex') || chr(10),
             'UTF8'
           ),
           'gzip',now(),now()
         )",
    )
    .bind(&object_id)
    .bind(chapter)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$1::UUID WHERE id=$2")
        .bind(&object_id)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires isolated disposable tor_ci"]
async fn h_ui_20_package_link_requires_current_published_paid_and_age_access() {
    let (pool, repo, owner, buyer, book) = setup("h20-link-visible").await;
    let (layer, version, _) = offer(&repo, owner, book, 125, true).await;
    let (chapter, edition) = paid(&pool, book, 1).await;
    let (unattached, _) = paid(&pool, book, 2).await;

    assert!(exclusive_package_link(&pool, book, None, Some(buyer))
        .await.unwrap().is_none(), "direct sales have no package-only link");
    repo.set_package_paid_mode(
        id(owner), id(layer), id(book), id(version), 0, PackagePaidMode::PackageOnly,
    ).await.unwrap();
    assert!(exclusive_package_link(&pool, book, Some(chapter), Some(buyer))
        .await.unwrap().is_none(), "unattached editions must not advertise purchase");
    attach_test_ciphertext(&pool, chapter, edition).await;

    let link = exclusive_package_link(&pool, book, Some(chapter), Some(buyer))
        .await.unwrap().unwrap();
    assert_eq!(link.layer_id, layer);
    assert_eq!(link.title, "Пакет из своих книг");
    assert_eq!(link.until_utc, None);
    assert_eq!(
        exclusive_package_for_chapter(&pool, chapter, Some(buyer)).await.unwrap(),
        Some((book, link.clone())),
    );
    assert_eq!(
        exclusive_package_link(&pool, book, None, None).await.unwrap(),
        Some(link.clone()),
        "basic public books may show the public package even to guests",
    );
    for bad in [
        exclusive_package_link(&pool, book, Some(unattached), Some(buyer)).await.unwrap(),
        exclusive_package_link(&pool, book, Some(-1), Some(buyer)).await.unwrap(),
        exclusive_package_link(&pool, book + 100_000, Some(chapter), Some(buyer)).await.unwrap(),
    ] {
        assert!(bad.is_none());
    }
    assert!(exclusive_package_for_chapter(&pool, -1, Some(buyer))
        .await.unwrap().is_none());

    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book).execute(&pool).await.unwrap();
    sqlx::query("UPDATE users SET adult_enabled=FALSE WHERE id=$1")
        .bind(buyer).execute(&pool).await.unwrap();
    assert!(exclusive_package_link(&pool, book, None, Some(buyer))
        .await.unwrap().is_none(), "age-denied users must not see the package");
    assert!(exclusive_package_link(&pool, book, None, None)
        .await.unwrap().is_none(), "guest must not see adult package");
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer).execute(&pool).await.unwrap();
    assert!(exclusive_package_link(&pool, book, Some(chapter), Some(buyer))
        .await.unwrap().is_some());

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book).execute(&pool).await.unwrap();
    assert!(exclusive_package_link(&pool, book, None, Some(buyer))
        .await.unwrap().is_none(), "held books must never advertise a package");
    let balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    ).bind(buyer).fetch_one(&pool).await.unwrap();
    assert_eq!(balance, 1500, "read-only navigation cannot debit the wallet");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated disposable tor_ci"]
async fn h_ui_20_package_link_follows_early_expiry_and_hiding() {
    let (pool, repo, owner, buyer, book) = setup("h20-link-expiry").await;
    let (layer, version, _) = offer(&repo, owner, book, 125, false).await;
    let (chapter, edition) = paid(&pool, book, 1).await;
    attach_test_ciphertext(&pool, chapter, edition).await;
    repo.set_package_paid_mode(
        id(owner), id(layer), id(book), id(version), 0, PackagePaidMode::First7Days,
    ).await.unwrap();

    let timed = exclusive_package_link(&pool, book, Some(chapter), Some(buyer))
        .await.unwrap().unwrap();
    assert_eq!(timed.layer_id, layer);
    assert!(timed.until_utc.is_some());
    sqlx::query("UPDATE books SET package_only_until=now()-interval '1 second' WHERE id=$1")
        .bind(book).execute(&pool).await.unwrap();
    assert!(exclusive_package_link(&pool, book, Some(chapter), Some(buyer))
        .await.unwrap().is_none(), "expired early window must restore direct sale");
    // Re-enable after expiry, then hide the package: do not link to a 404 storefront.
    let revision: i64 = sqlx::query_scalar(
        "SELECT package_only_revision FROM books WHERE id=$1",
    ).bind(book).fetch_one(&pool).await.unwrap();
    repo.set_package_paid_mode(
        id(owner), id(layer), id(book), id(version), revision, PackagePaidMode::PackageOnly,
    ).await.unwrap();
    sqlx::query("UPDATE commerce_layers SET status='hidden' WHERE id=$1")
        .bind(layer).execute(&pool).await.unwrap();
    assert!(exclusive_package_link(&pool, book, Some(chapter), Some(buyer))
        .await.unwrap().is_none(), "hidden packages never produce a paid link");
    pool.close().await;
}
