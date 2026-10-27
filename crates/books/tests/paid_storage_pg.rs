//! B S3-09/11: owner-only historical paid version to verified G object.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::paid_storage::{PaidPublication, PgPaidStorage},
    services::authoring::AuthorError,
};
use tor_contracts::paid_delivery::{ChapterVersionPort, ChapterVersionRef, ObjectId, PositiveId};

fn pinned(chapter: i64, version: i64) -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    }
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// G is tested against real MinIO in S3-04/05. These are G-verified input
/// fixtures for exercising B's atomic attach and historical retention on PG.
async fn ready_object(
    pool: &PgPool,
    chapter: i64,
    version: i64,
    body: &str,
    raw_id: &str,
    corrupt_plaintext_hash: bool,
    kek_id: &str,
) -> ObjectId {
    let id = ObjectId::parse(raw_id).unwrap();
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let digest = if corrupt_plaintext_hash {
        [99_u8; 32]
    } else {
        sha(normalized.as_bytes())
    };
    let aad = format!(
        "tor.encrypted.chapter.v1\n{chapter}\n{version}\n{}\n{}\n",
        id.as_str(),
        hex::encode(digest),
    );
    sqlx::query(
        "INSERT INTO storage_objects
          (id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
           kek_id,object_nonce,object_aad,codec,verified_at,delete_after)
         VALUES ($1::uuid,'chapter','tor-private-chapter-ci',$2,'ready',64,
                 $3,$4,$5,$6,$9,$7,$8,'gzip',
                 now(),now()+INTERVAL '24 hours')",
    )
    .bind(id.as_str())
    .bind(format!("private/chapter-versions/{}.enc", id.as_str()))
    .bind([4_u8; 32].as_slice())
    .bind(digest.as_slice())
    .bind([8_u8; 48].as_slice())
    .bind([3_u8; 12].as_slice())
    .bind([2_u8; 12].as_slice())
    .bind(aad.as_bytes())
    .bind(kek_id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn stored_object(pool: &PgPool, id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT storage_object_id::text FROM chapter_versions WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn object_state(pool: &PgPool, id: &ObjectId) -> String {
    sqlx::query_scalar("SELECT state FROM storage_objects WHERE id=$1::uuid")
        .bind(id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PG18 TEST_DATABASE_URL ending /tor_ci"]
async fn b_04_s3_09_11_verified_paid_v1_remains_after_v2_publish() {
    let url = std::env::var("TEST_DATABASE_URL").expect("CI disposable database");
    assert!(
        url.ends_with("/tor_ci"),
        "never connect production database"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let b = PgPaidStorage::new(pool.clone());
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b-s3-owner@example.invalid','B S3 Owner','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let buyer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('b-s3-buyer@example.invalid','B S3 Buyer','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:paid-storage-b09','ru','History of paid versions','public',$1)
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Paid book chapter','public','paid',100)
         RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1_body = "Купленная V1 \r\n А😀Б";
    let v2_body = "Новая V2: 第二章";
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(v1_body)
    .bind(sha(v1_body.as_bytes()).as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,2,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(v2_body)
    .bind(sha(v2_body.as_bytes()).as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    let purchase: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
           (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,'b-s3-paid-v1-settlement',100,'RUB') RETURNING id",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();

    let object_v1 = ready_object(
        &pool,
        chapter,
        v1,
        v1_body,
        "b9040000-0001-4000-8000-000000000001",
        false,
        "content-kek-v1",
    )
    .await;
    let object_wrong = ready_object(
        &pool,
        chapter,
        v2,
        v2_body,
        "b9040000-0002-4000-8000-000000000002",
        true,
        "content-kek-v2",
    )
    .await;
    let object_v2 = ready_object(
        &pool,
        chapter,
        v2,
        v2_body,
        "b9040000-0003-4000-8000-000000000003",
        false,
        "content-kek-v2",
    )
    .await;

    // An outsider cannot attach another author's object, even if it is ready.
    assert_eq!(
        b.attach_verified_and_publish(
            buyer,
            book,
            pinned(chapter, v1),
            &object_v1,
            PaidPublication::KeepCurrent
        )
        .await,
        Err(AuthorError::Missing)
    );
    assert_eq!(object_state(&pool, &object_v1).await, "ready");
    assert_eq!(stored_object(&pool, v1).await, None);
    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v1),
            &object_v1,
            PaidPublication::KeepCurrent
        )
        .await,
        Ok(())
    );
    assert_eq!(object_state(&pool, &object_v1).await, "attached");
    assert_eq!(
        stored_object(&pool, v1).await.as_deref(),
        Some(object_v1.as_str())
    );
    // B-only metadata: exact UTF-16 lengths, no body or S3 credentials.
    let (lengths, last_block, last_offset, content_sha): (Vec<i32>, i32, i32, Vec<u8>) =
        sqlx::query_as(
            "SELECT block_utf16_lengths,last_readable_block_index,
                    last_readable_char_offset,content_sha256
             FROM chapter_reader_manifests
             WHERE chapter_id=$1 AND version_id=$2 AND storage_object_id=$3::uuid",
        )
        .bind(chapter)
        .bind(v1)
        .bind(object_v1.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    let normalized_v1 = v1_body.replace("\r\n", "\n").replace('\r', "\n");
    assert_eq!(lengths, vec![normalized_v1.encode_utf16().count() as i32]);
    assert_eq!((last_block, last_offset), (1, lengths[0]));
    assert_eq!(content_sha, sha(normalized_v1.as_bytes()).to_vec());
    let before_replay: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_reader_manifests WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before_replay, 1);

    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v1),
            &object_v1,
            PaidPublication::KeepCurrent
        )
        .await,
        Ok(()),
        "exact attach retry is idempotent"
    );
    let after_replay: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_reader_manifests WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_replay, 1);
    assert!(
        sqlx::query(
            "UPDATE chapter_reader_manifests SET parser_schema=1
             WHERE chapter_id=$1 AND version_id=$2",
        )
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .is_err(),
        "a released reader manifest must not be edited in place"
    );

    // Even a forged G-ready object with the wrong plaintext SHA rolls back.
    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v2),
            &object_wrong,
            PaidPublication::Publish {
                expected_current: Some(v1)
            }
        )
        .await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(object_state(&pool, &object_wrong).await, "ready");
    assert_eq!(stored_object(&pool, v2).await, None);
    let invalid_manifest_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_reader_manifests WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        invalid_manifest_count, 0,
        "bad ciphertext may not mint a reader manifest"
    );

    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v2),
            &object_v2,
            PaidPublication::Publish {
                expected_current: Some(v1 + v2 + 900_000)
            }
        )
        .await,
        Err(AuthorError::Conflict),
        "stale publish must not consume ready ciphertext"
    );
    assert_eq!(object_state(&pool, &object_v2).await, "ready");

    // Legitimate V2 commits object attach, pointer switch and outbox together.
    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v2),
            &object_v2,
            PaidPublication::Publish {
                expected_current: Some(v1)
            }
        )
        .await,
        Ok(())
    );
    assert_eq!(object_state(&pool, &object_v2).await, "attached");
    assert_eq!(
        stored_object(&pool, v2).await.as_deref(),
        Some(object_v2.as_str())
    );
    let manifests: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_reader_manifests
         WHERE chapter_id=$1 AND version_id IN ($2,$3)",
    )
    .bind(chapter)
    .bind(v1)
    .bind(v2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        manifests, 2,
        "V2 must never erase purchased V1 parser metadata"
    );

    let current: i64 =
        sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
            .bind(chapter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, v2);
    let event: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_encrypted_release_events
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(v2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event, 1);
    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v2),
            &object_v2,
            PaidPublication::Publish {
                expected_current: Some(v1)
            }
        )
        .await,
        Ok(()),
        "exact V2 replay cannot duplicate B release events"
    );
    assert_eq!(
        b.attach_verified_and_publish(
            owner,
            book,
            pinned(chapter, v1),
            &object_v1,
            PaidPublication::Publish {
                expected_current: Some(v1)
            }
        )
        .await,
        Err(AuthorError::Invalid),
        "client cannot request a backwards move to its own old version"
    );

    // D's historical purchase remains exact V1, independent of current V2.
    let still_owned: bool = sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
        .bind(buyer)
        .bind(purchase)
        .bind(chapter)
        .bind(v1)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(still_owned);
    let v1_metadata = b
        .encrypted_chapter(pinned(chapter, v1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v1_metadata.object.object_id, object_v1);
    assert_eq!(v1_metadata.wrapped_key_id, "content-kek-v1");
    let v2_metadata = b
        .encrypted_chapter(pinned(chapter, v2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v2_metadata.object.object_id, object_v2);
    assert_eq!(v2_metadata.wrapped_key_id, "content-kek-v2");
    assert_ne!(v1_metadata.aad, v2_metadata.aad);

    let version_mutation =
        sqlx::query("UPDATE chapter_versions SET body_markdown='tampered' WHERE id=$1")
            .bind(v1)
            .execute(&pool)
            .await;
    assert!(version_mutation.is_err());
    let object_mutation =
        sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
            .bind(v1)
            .bind(object_v2.as_str())
            .execute(&pool)
            .await;
    assert!(object_mutation.is_err());
    let deletion = sqlx::query("DELETE FROM storage_objects WHERE id=$1::uuid")
        .bind(object_v1.as_str())
        .execute(&pool)
        .await;
    assert!(deletion.is_err(), "purchased historical V1 cannot be GC'd");

    // D contract: creator hiding a storefront never revokes a settled V1.
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='draft' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    let still_allowed: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(buyer)
            .bind(purchase)
            .bind(chapter)
            .bind(v1)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(still_allowed, "creator-hide never revokes the paid V1");
    assert_eq!(
        b.encrypted_chapter(pinned(chapter, v1))
            .await
            .unwrap()
            .unwrap()
            .object
            .object_id,
        object_v1,
        "B must not deny the historical V1 permitted by D"
    );
    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();

    // Administrative moderation still overrides a paid entitlement.

    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert!(b
        .encrypted_chapter(pinned(chapter, v1))
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE chapters SET admin_hold=FALSE WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
