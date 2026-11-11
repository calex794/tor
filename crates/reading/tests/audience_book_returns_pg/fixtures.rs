//! Local fixtures for the per-book contract; no production history is backfilled.
use sqlx::PgPool;
use tor_reading::{
    repositories::{returns::PgReadingReturns, sync::PurchasedFinish, PgLibrary, PgReadingSync},
    services::{progress::ReadingPlace, sync::SyncError},
};

#[derive(Clone, Copy)]
pub(super) struct Book {
    pub(super) id: i64,
    pub(super) chapter: i64,
    pub(super) version: i64,
}

pub(super) fn place(offset: i32) -> ReadingPlace {
    ReadingPlace {
        block_index: 1,
        char_offset: offset,
    }
}

pub(super) async fn actor(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Return fixture','$argon2id$fixture') RETURNING id")
        .bind(format!("book-returns-{name}-{}@example.invalid", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
        .fetch_one(pool).await.unwrap()
}

pub(super) async fn consent(pool: &PgPool, actor: i64, enabled: bool) -> String {
    sqlx::query_scalar(
        "INSERT INTO rec_subjects(user_id,enabled,enabled_at)
         VALUES($1,$2,CASE WHEN $2 THEN now() ELSE NULL END)
         ON CONFLICT(user_id) DO UPDATE SET subject_id=gen_random_uuid(),
           enabled=$2,enabled_at=CASE WHEN $2 THEN now() ELSE NULL END,revision=rec_subjects.revision+1
         RETURNING subject_id::text",
    ).bind(actor).bind(enabled).fetch_one(pool).await.unwrap()
}

pub(super) async fn book(pool: &PgPool, name: &str, paid: bool, books: &mut Vec<i64>) -> Book {
    let id = sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES($1,'ru','Return fixture','public','basic') RETURNING id")
        .bind(format!("ci:book-returns:{name}:{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
        .fetch_one(pool).await.unwrap();
    let chapter = sqlx::query_scalar("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES($1,1,'Chapter','public',$2,$3) RETURNING id")
        .bind(id).bind(if paid {"paid"} else {"free"}).bind(if paid {100_i64} else {0})
        .fetch_one(pool).await.unwrap();
    let version = sqlx::query_scalar("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES($1,1,'abcdefghij') RETURNING id")
        .bind(chapter).fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    books.push(id);
    Book {
        id,
        chapter,
        version,
    }
}

async fn purchase(pool: &PgPool, actor: i64, book: Book) -> i64 {
    sqlx::query_scalar("INSERT INTO chapter_purchase_facts(user_id,chapter_id,version_id,settlement_reference,amount_minor,currency) VALUES($1,$2,$3,$4,100,'RUB') RETURNING id")
        .bind(actor).bind(book.chapter).bind(book.version).bind(format!("ci:return:{actor}:{}", book.chapter))
        .fetch_one(pool).await.unwrap()
}

pub(super) async fn episodes(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reader_book_return_subjects r JOIN rec_subjects s USING(subject_id) WHERE s.user_id=$1")
        .bind(actor).fetch_one(pool).await.unwrap()
}

pub(super) async fn cohort(pool: &PgPool, actor: i64, book: i64, age: i32, flags: [bool; 2]) {
    sqlx::query("INSERT INTO reader_book_return_subjects(subject_id,book_id,cohort_day,returned_d7,returned_d30) SELECT subject_id,$2,(now() AT TIME ZONE 'UTC')::date-$3::int,$4,$5 FROM rec_subjects WHERE user_id=$1")
        .bind(actor).bind(book).bind(age).bind(flags[0]).bind(flags[1]).execute(pool).await.unwrap();
}

pub(super) async fn active(pool: &PgPool) -> (i64, String) {
    sqlx::query_as(
        "SELECT id,started_at::text FROM reader_book_return_coverage WHERE ended_at IS NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

pub(super) async fn paid_and_archived_cases(
    pool: &PgPool,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    let buyer = actor(pool, "paid").await;
    actors.push(buyer);
    let stranger = actor(pool, "stranger").await;
    actors.push(stranger);
    consent(pool, buyer, true).await;
    consent(pool, stranger, true).await;
    let paid = book(pool, "paid", true, books).await;
    let paid_id = purchase(pool, buyer, paid).await;
    sync.save_purchased(buyer, paid_id, paid.chapter, paid.version, place(2), 0)
        .await
        .unwrap();
    assert_eq!(episodes(pool, buyer).await, 1);
    assert_eq!(
        sync.save_purchased(stranger, paid_id, paid.chapter, paid.version, place(3), 0)
            .await,
        Err(SyncError::NotFound)
    );
    sqlx::query("INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason) VALUES($1,$2,'refund')")
        .bind(paid_id).bind(format!("ci:return-refund:{paid_id}")).execute(pool).await.unwrap();
    assert_eq!(
        sync.save_purchased(buyer, paid_id, paid.chapter, paid.version, place(3), 1)
            .await,
        Err(SyncError::NotFound)
    );

    let archived = book(pool, "archived", false, books).await;
    let archive_reader = actor(pool, "archive").await;
    actors.push(archive_reader);
    consent(pool, archive_reader, true).await;
    PgLibrary::new(pool.clone())
        .add_book(archive_reader, archived.id)
        .await
        .unwrap();
    let mut digest = [0_u8; 32];
    digest[..8].copy_from_slice(&archive_reader.to_le_bytes());
    digest[31] = 19;
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at) VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(archive_reader)
    .bind(digest.as_slice())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='pending_deletion' WHERE id=$1")
        .bind(archived.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET lifecycle_status='ordinary_deleted',ordinary_deleted_at=now(),visibility='draft' WHERE id=$1")
        .bind(archived.id).execute(pool).await.unwrap();
    sync.save_archived_free(
        archive_reader,
        archived.chapter,
        archived.version,
        place(2),
        0,
        &digest,
    )
    .await
    .unwrap();
    assert_eq!(
        episodes(pool, archive_reader).await,
        1,
        "allowed pinned reading is observed without public metadata"
    );
    assert!(
        PgReadingReturns::new(pool.clone())
            .book_returns_report(archived.id, 90)
            .await
            .unwrap()
            .is_none(),
        "private archived metadata is never reported"
    );
    let starts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_chapter_starts WHERE user_id=$1")
            .bind(archive_reader)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        starts, 0,
        "archive day does not invent canonical first-start events"
    );
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(digest.as_slice())
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        sync.save_archived_free(
            archive_reader,
            archived.chapter,
            archived.version,
            place(3),
            1,
            &digest
        )
        .await,
        Err(SyncError::NotFound)
    );
    encrypted_case(pool, sync, actors, books).await;
}

async fn encrypted_case(
    pool: &PgPool,
    sync: &PgReadingSync,
    actors: &mut Vec<i64>,
    books: &mut Vec<i64>,
) {
    let buyer = actor(pool, "encrypted").await;
    actors.push(buyer);
    consent(pool, buyer, true).await;
    let paid = book(pool, "encrypted", true, books).await;
    let object: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO storage_objects(id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,kek_id,object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES($1::uuid,'chapter','tor-private-return-ci','private/chapter-versions/'||$1::text||'.enc','attached',64,
           decode(repeat('11',32),'hex'),sha256(convert_to('abcdefghij','UTF8')),decode(repeat('22',48),'hex'),decode(repeat('33',12),'hex'),
           'content-kek-v1',decode(repeat('44',12),'hex'),convert_to(concat('tor.encrypted.chapter.v1',chr(10),$2::text,chr(10),$3::text,chr(10),$1::text,chr(10),encode(sha256(convert_to('abcdefghij','UTF8')),'hex'),chr(10)),'UTF8'),'gzip',now(),now())",
    ).bind(&object).bind(paid.chapter).bind(paid.version).execute(pool).await.unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(paid.version)
        .bind(&object)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chapter_reader_manifests(chapter_id,version_id,storage_object_id,content_sha256,block_utf16_lengths,last_readable_block_index,last_readable_char_offset) VALUES($1,$2,$3::uuid,sha256(convert_to('abcdefghij','UTF8')),ARRAY[10],1,10)")
        .bind(paid.chapter).bind(paid.version).bind(&object).execute(pool).await.unwrap();
    let hash: String =
        sqlx::query_scalar("SELECT encode(sha256(convert_to('abcdefghij','UTF8')),'hex')")
            .fetch_one(pool)
            .await
            .unwrap();
    let paid_id = purchase(pool, buyer, paid).await;
    sync.save_purchased(buyer, paid_id, paid.chapter, paid.version, place(1), 0)
        .await
        .unwrap();
    assert_eq!(
        episodes(pool, buyer).await,
        0,
        "encrypted shadow position alone is not verified reading"
    );
    let command = || PurchasedFinish {
        purchase_id: paid_id,
        chapter_id: paid.chapter,
        version_id: paid.version,
        place: place(10),
        expected_revision: 1,
        plaintext_sha256: &hash,
    };
    assert!(
        sync.complete_purchased_s3(buyer, command())
            .await
            .unwrap()
            .1
    );
    assert_eq!(episodes(pool, buyer).await, 1);
    consent(pool, buyer, false).await;
    consent(pool, buyer, true).await;
    assert!(
        !sync
            .complete_purchased_s3(buyer, command())
            .await
            .unwrap()
            .1
    );
    assert_eq!(
        episodes(pool, buyer).await,
        0,
        "replayed encrypted completion cannot create a new consented day"
    );
}
