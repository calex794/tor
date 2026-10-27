//! B/S065: exact version-scoped images, real restricted PostgreSQL 18.
//! Metadata below models a G-verified ready receipt, not a working image
//! upload; the actual S3 provider is tested independently by G's suite.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::repositories::{
    paid_assets::PgPaidAssets,
    paid_storage::{PaidPublication, PgPaidStorage},
};
use tor_books::services::authoring::AuthorError;
use tor_contracts::paid_delivery::{ChapterVersionRef, ContentCodec, ObjectId, PositiveId};

fn pinned(chapter: i64, version: i64) -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    }
}

fn digest(input: &[u8]) -> [u8; 32] {
    Sha256::digest(input).into()
}

async fn ready_asset(pool: &PgPool, pinned: ChapterVersionRef, id: &str) -> ObjectId {
    let object = ObjectId::parse(id).unwrap();
    let plaintext_sha = digest(b"PNG fixture only; G verifies actual image elsewhere");
    let aad = format!(
        "tor.encrypted.asset.v1\n{}\n{}\n{}\n{}\n",
        pinned.chapter_id.get(),
        pinned.version_id.get(),
        object.as_str(),
        hex::encode(plaintext_sha),
    );
    sqlx::query(
        "INSERT INTO storage_objects
         (id,object_kind,bucket,object_key,state,size_bytes,
          sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
          kek_id,object_nonce,object_aad,codec,verified_at,delete_after)
         VALUES($1::uuid,'chapter_asset','tor-private-chapter-ci',$2,
                'ready',64,$3,$4,$5,$6,'content-kek-v1',
                $7,$8,'none',now(),now()+INTERVAL '24 hours')",
    )
    .bind(object.as_str())
    .bind(format!("private/chapter-assets/{}.enc", object.as_str()))
    .bind([4_u8; 32].as_slice())
    .bind(plaintext_sha.as_slice())
    .bind([8_u8; 48].as_slice())
    .bind([3_u8; 12].as_slice())
    .bind([2_u8; 12].as_slice())
    .bind(aad.as_bytes())
    .execute(pool)
    .await
    .unwrap();
    object
}

async fn ready_chapter(pool: &PgPool, pinned: ChapterVersionRef, body: &str, id: &str) -> ObjectId {
    let object = ObjectId::parse(id).unwrap();
    let plaintext_sha = digest(body.as_bytes());
    let aad = format!(
        "tor.encrypted.chapter.v1\n{}\n{}\n{}\n{}\n",
        pinned.chapter_id.get(),
        pinned.version_id.get(),
        object.as_str(),
        hex::encode(plaintext_sha),
    );
    sqlx::query(
        "INSERT INTO storage_objects
         (id,object_kind,bucket,object_key,state,size_bytes,
          sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
          kek_id,object_nonce,object_aad,codec,verified_at,delete_after)
         VALUES($1::uuid,'chapter','tor-private-chapter-ci',$2,'ready',64,
                $3,$4,$5,$6,'content-kek-v1',$7,$8,'gzip',
                now(),now()+INTERVAL '24 hours')",
    )
    .bind(object.as_str())
    .bind(format!("private/chapter-versions/{}.enc", object.as_str()))
    .bind([4_u8; 32].as_slice())
    .bind(plaintext_sha.as_slice())
    .bind([8_u8; 48].as_slice())
    .bind([3_u8; 12].as_slice())
    .bind([2_u8; 12].as_slice())
    .bind(aad.as_bytes())
    .execute(pool)
    .await
    .unwrap();
    object
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 TEST_DATABASE_URL ending /tor_ci"]
async fn b_s065_paid_image_stays_with_exact_purchased_v1_and_never_mutates() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
    assert!(url.ends_with("/tor_ci"), "never use production PostgreSQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let assets = PgPaidAssets::new(pool.clone());
    let versions = PgPaidStorage::new(pool.clone());
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b-s065-owner@example.invalid','S065 Owner','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let outsider: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b-s065-other@example.invalid','S065 Other','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:b-s065-private-images','ru','S065 private images','public',$1)
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Protected images','draft','paid',125) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let first = "Paid V1 body";
    let second = "Paid V2 body";
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(first)
    .bind(digest(first.as_bytes()).as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,2,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(second)
    .bind(digest(second.as_bytes()).as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1 = pinned(chapter, v1);
    let v2 = pinned(chapter, v2);
    let image_v1 = ready_asset(&pool, v1, "b0b0a500-0001-4000-8000-000000000001").await;
    let image_other = ready_asset(&pool, v1, "b0b0a500-0002-4000-8000-000000000002").await;

    assert_eq!(
        assets
            .attach_prepared(outsider, v1, 1, "image/png", &image_v1)
            .await,
        Err(AuthorError::Missing),
        "foreign author cannot attach images"
    );
    assert_eq!(
        assets
            .attach_prepared(owner, v2, 1, "image/png", &image_v1)
            .await,
        Err(AuthorError::Conflict),
        "V1 AAD may not be relabeled as V2"
    );
    assert_eq!(
        assets
            .attach_prepared(owner, v1, 0, "image/png", &image_v1)
            .await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        assets
            .attach_prepared(owner, v1, 1, "text/html", &image_v1)
            .await,
        Err(AuthorError::Invalid)
    );
    assert_eq!(
        assets.encrypted_for_pinned(v1, &image_v1).await.unwrap(),
        None
    );

    assert_eq!(
        assets
            .attach_prepared(owner, v1, 1, "image/png", &image_v1)
            .await,
        Ok(())
    );
    assert_eq!(
        assets
            .attach_prepared(owner, v1, 1, "image/png", &image_v1)
            .await,
        Ok(()),
        "one safe idempotent binding replay"
    );
    assert_eq!(
        assets
            .attach_prepared(owner, v1, 1, "image/png", &image_other)
            .await,
        Err(AuthorError::Conflict),
        "immutable ordinal cannot be changed"
    );
    let state: (String, bool) =
        sqlx::query_as("SELECT state,delete_after IS NULL FROM storage_objects WHERE id=$1::uuid")
            .bind(image_v1.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("attached".to_owned(), true));
    assert_eq!(
        assets.encrypted_for_pinned(v1, &image_v1).await.unwrap(),
        None,
        "an unpublished image cannot be resolved as a readable version"
    );

    let chapter_v1 = ready_chapter(&pool, v1, first, "b0b0a500-0003-4000-8000-000000000003").await;
    versions
        .attach_verified_and_publish(
            owner,
            book,
            v1,
            &chapter_v1,
            PaidPublication::Publish {
                expected_current: None,
            },
        )
        .await
        .unwrap();

    let reference = assets
        .encrypted_for_pinned(v1, &image_v1)
        .await
        .unwrap()
        .expect("published exact V1 retains one private image");
    assert_eq!(
        reference.object.kind,
        tor_contracts::paid_delivery::EncryptedObjectKind::ChapterAsset
    );
    assert_eq!(reference.codec, ContentCodec::None);
    assert_eq!(reference.object.object_id, image_v1);
    assert_eq!(
        assets.encrypted_for_pinned(v2, &image_v1).await.unwrap(),
        None
    );
    assert_eq!(
        assets
            .attach_prepared(owner, v1, 2, "image/png", &image_other)
            .await,
        Err(AuthorError::Conflict),
        "not even the owner can add an image after publication"
    );
    assert_eq!(
        sqlx::query(
            "UPDATE chapter_version_assets SET version_id=$1
             WHERE chapter_id=$2 AND storage_object_id=$3::uuid",
        )
        .bind(v2.version_id.get())
        .bind(chapter)
        .bind(image_v1.as_str())
        .execute(&pool)
        .await
        .is_err(),
        true,
        "direct SQL cannot move an image between historical editions"
    );
    assert!(
        sqlx::query("DELETE FROM chapter_version_assets WHERE storage_object_id=$1::uuid")
            .bind(image_v1.as_str())
            .execute(&pool)
            .await
            .is_err(),
        "an accepted image cannot be deleted from purchased V1"
    );
    let chapter_v2 = ready_chapter(&pool, v2, second, "b0b0a500-0004-4000-8000-000000000004").await;
    versions
        .attach_verified_and_publish(
            owner,
            book,
            v2,
            &chapter_v2,
            PaidPublication::Publish {
                expected_current: Some(v1.version_id.get()),
            },
        )
        .await
        .unwrap();
    assert!(
        assets
            .encrypted_for_pinned(v1, &image_v1)
            .await
            .unwrap()
            .is_some(),
        "V2 publication must not revoke verified V1 image membership"
    );
    assert_eq!(
        assets.encrypted_for_pinned(v2, &image_v1).await.unwrap(),
        None
    );
    pool.close().await;
}
