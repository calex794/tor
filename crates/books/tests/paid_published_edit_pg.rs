//! BKR-96: paid published text edit under real PostgreSQL, with independent
//! immutable paid versions and an optimistic chapter pointer/revision CAS.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::paid_storage::{PaidPublication, PgPaidStorage},
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};
use tor_contracts::paid_delivery::{ChapterVersionPort, ChapterVersionRef, ObjectId, PositiveId};

fn pinned(chapter: i64, version: i64) -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    }
}

fn request(
    book: i64,
    chapter: i64,
    revision: i64,
    key: &str,
    body: &str,
) -> tor_books::services::authoring::SaveChapter {
    save_chapter(SaveChapterRequest {
        book_id: u64::try_from(book).unwrap(),
        chapter_id: Some(u64::try_from(chapter).unwrap()),
        number: 1,
        expected_revision: revision,
        title: "Платная глава",
        body,
        access: "paid",
        price_amount: 100,
        command_key: key,
    })
    .unwrap()
}

// This is a G-verified registry fixture; the real private S3 PUT/GET/grant
// path is independently exercised by the paid-author integration pipeline.
async fn ready_object(
    pool: &PgPool,
    chapter: i64,
    version: i64,
    body: &str,
    raw_id: &str,
) -> ObjectId {
    let id = ObjectId::parse(raw_id).unwrap();
    let digest: [u8; 32] = Sha256::digest(body.as_bytes()).into();
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
         VALUES($1::uuid,'chapter','tor-private-chapter-ci',$2,'ready',64,
                $3,$4,$5,$6,'content-kek-v2',$7,$8,'gzip',
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
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn paid_edit_keeps_purchased_v1_and_rejects_stale_race_and_reused_key() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG18 CI");
    assert_eq!(
        url,
        "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
        "production database forbidden"
    );
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let pool = PgPool::connect(&url).await.unwrap();
    let repo = PgPaidStorage::new(pool.clone());
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('paid-edit-owner@example.invalid','Paid editor','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('paid-edit-stranger@example.invalid','Paid stranger','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:bkr-paid-edit','ru','Paid edit book','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'Платная глава','public','paid',100,1) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let old_body = "Купленная V1 — текст навсегда";
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,$2,$3) RETURNING id",
    )
    .bind(chapter)
    .bind(old_body)
    .bind(Sha256::digest(old_body.as_bytes()).as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let object_v1 = ready_object(
        &pool, chapter, v1, old_body,
        "f034f034-0001-4001-8001-000000000341",
    )
    .await;
    repo.attach_verified_and_publish(
        owner,
        book,
        pinned(chapter, v1),
        &object_v1,
        PaidPublication::Publish {
            expected_current: None,
        },
    )
    .await
    .unwrap();
    let original_first: i64 = sqlx::query_scalar(
        "SELECT version_id FROM chapter_first_release_events WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(original_first, v1);
    let original_version: (String, Option<String>) = sqlx::query_as(
        "SELECT body_markdown,storage_object_id::text FROM chapter_versions WHERE id=$1",
    )
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();

    let key1 = "e".repeat(43);
    let key2 = "f".repeat(43);
    let first = request(book, chapter, 1, &key1, "Новая V2 — 🦈");
    let second = request(book, chapter, 1, &key2, "Конкурирующая V2");
    assert_eq!(
        repo.prepare_published_paid_edit(stranger, &first).await,
        Err(AuthorError::Missing),
    );
    let v2 = repo
        .prepare_published_paid_edit(owner, &first)
        .await
        .unwrap();
    assert!(!v2.published);
    assert_eq!(v2.previous_version_id, v1);
    assert_eq!(
        repo.prepare_published_paid_edit(owner, &first).await,
        Ok(v2),
        "same-key retry preserves version identity",
    );
    let bad_body = request(book, chapter, 1, &key1, "Подменённый текст");
    assert_eq!(
        repo.prepare_published_paid_edit(owner, &bad_body).await,
        Err(AuthorError::Conflict),
    );
    let rival = repo
        .prepare_published_paid_edit(owner, &second)
        .await
        .unwrap();
    assert_ne!(rival.version_id, v2.version_id);
    let before: (i64, i64) = sqlx::query_as(
        "SELECT p.version_id,c.edit_revision FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, (v1, 1), "pending edits never change public pointer");

    let object_v2 = ready_object(
        &pool, chapter, v2.version_id, "Новая V2 — 🦈",
        "f034f034-0002-4002-8002-000000000342",
    )
    .await;
    let object_rival = ready_object(
        &pool, chapter, rival.version_id, "Конкурирующая V2",
        "f034f034-0003-4003-8003-000000000343",
    )
    .await;
    repo.attach_verified_and_publish(
        owner, book, pinned(chapter, v2.version_id), &object_v2,
        PaidPublication::PublishEdit {
            expected_current: v1,
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    let after: (i64, i64) = sqlx::query_as(
        "SELECT p.version_id,c.edit_revision FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, (v2.version_id, 2));
    // Exact HTTP retry doesn't create V3 or bump revision a second time.
    assert!(repo.prepare_published_paid_edit(owner, &first).await.unwrap().published);
    repo.attach_verified_and_publish(
        owner, book, pinned(chapter, v2.version_id), &object_v2,
        PaidPublication::PublishEdit {
            expected_current: v1,
            expected_revision: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        repo.attach_verified_and_publish(
            owner, book, pinned(chapter, rival.version_id), &object_rival,
            PaidPublication::PublishEdit {
                expected_current: v1,
                expected_revision: 1,
            },
        ).await,
        Err(AuthorError::Conflict),
        "stale other tab may not replace the winning published edition"
    );
    let unchanged: (i64, i64) = sqlx::query_as(
        "SELECT p.version_id,c.edit_revision FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unchanged, after);
    let later = request(book, chapter, 1, &"g".repeat(43), "Поздний текст");
    assert_eq!(
        repo.prepare_published_paid_edit(owner, &later).await,
        Err(AuthorError::Conflict),
    );
    let first_release: i64 = sqlx::query_scalar(
        "SELECT version_id FROM chapter_first_release_events WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first_release, v1);
    let historical: (String, Option<String>) = sqlx::query_as(
        "SELECT body_markdown,storage_object_id::text FROM chapter_versions WHERE id=$1",
    )
    .bind(v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(historical, original_version);
    assert_eq!(historical.0, old_body);
    assert!(repo.encrypted_chapter(pinned(chapter, v1)).await.unwrap().is_some());
    assert!(repo
        .encrypted_chapter(pinned(chapter, v2.version_id))
        .await
        .unwrap()
        .is_some());
    let rival_state: String = sqlx::query_scalar(
        "SELECT state FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(object_rival.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rival_state, "ready", "failed CAS never attaches stale S3");
    pool.close().await;
}
