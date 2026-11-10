use super::*;

#[derive(Clone, Copy)]
pub struct Book {
    pub id: i64,
    pub chapter: i64,
    pub version: i64,
}

pub async fn pool(max: u32) -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never use a permanent database");
    PgPoolOptions::new()
        .max_connections(max)
        .connect(&url)
        .await
        .unwrap()
}

pub async fn actor(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('flow-c-'||$1||'-'||gen_random_uuid()||'@example.invalid',
                'FLOW capture fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn consent(pool: &PgPool, actor: i64, flow: bool) -> (String, String) {
    sqlx::query_as(
        "INSERT INTO rec_subjects(user_id,enabled,enabled_at,flow_epoch,flow_enabled_at)
         VALUES($1,true,now(),CASE WHEN $2 THEN gen_random_uuid() END,
                CASE WHEN $2 THEN now() END)
         ON CONFLICT(user_id) DO UPDATE SET enabled=true,enabled_at=now(),
           flow_epoch=CASE WHEN $2 THEN gen_random_uuid() END,
           flow_enabled_at=CASE WHEN $2 THEN now() END,
           revision=rec_subjects.revision+1
         RETURNING subject_id::text,COALESCE(flow_epoch::text,'')",
    )
    .bind(actor)
    .bind(flow)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn book(pool: &PgPool, paid: bool) -> Book {
    let id = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES('ci:flow-c:'||gen_random_uuid(),'ru','FLOW capture fixture','public','basic')
         RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let chapter = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Chapter','public',$2,$3) RETURNING id",
    )
    .bind(id)
    .bind(if paid { "paid" } else { "free" })
    .bind(if paid { 100_i64 } else { 0 })
    .fetch_one(pool)
    .await
    .unwrap();
    let version = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'abcdefghij') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    Book {
        id,
        chapter,
        version,
    }
}

pub fn place(offset: i32) -> ReadingPlace {
    ReadingPlace {
        block_index: 1,
        char_offset: offset,
    }
}

pub async fn actions(pool: &PgPool, actor: i64, kind: i16) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_verified_book_actions a
         JOIN rec_subjects s ON s.subject_id=a.subject_id AND s.flow_epoch=a.flow_epoch
         WHERE s.user_id=$1 AND a.kind=$2",
    )
    .bind(actor)
    .bind(kind)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn days(pool: &PgPool, actor: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_verified_activity_days a
         JOIN rec_subjects s ON s.subject_id=a.subject_id AND s.flow_epoch=a.flow_epoch
         WHERE s.user_id=$1",
    )
    .bind(actor)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn erase_scopes(pool: &PgPool, actors: &[i64]) {
    sqlx::query("DELETE FROM rec_subjects WHERE user_id=ANY($1)")
        .bind(actors)
        .execute(pool)
        .await
        .unwrap();
}

pub async fn reads(pool: &PgPool, actor: i64, book: i64) -> Vec<i16> {
    sqlx::query_scalar(
        "SELECT v.kind FROM reader_flow_verified_read_receipts p
         JOIN reader_flow_verified_read_events v USING(event_id)
         JOIN rec_subjects s ON s.subject_id=p.subject_id AND s.flow_epoch=p.flow_epoch
         WHERE s.user_id=$1 AND v.book_id=$2 ORDER BY v.kind",
    )
    .bind(actor)
    .bind(book)
    .fetch_all(pool)
    .await
    .unwrap()
}

pub async fn purchase(pool: &PgPool, actor: i64, book: Book) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
         (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,'ci:flow-c:'||gen_random_uuid(),100,'RUB') RETURNING id",
    )
    .bind(actor)
    .bind(book.chapter)
    .bind(book.version)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn encrypted(pool: &PgPool, paid: Book) -> String {
    let object: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO storage_objects(id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,kek_id,
           object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES($1::uuid,'chapter','tor-private-flow-ci',
           'private/chapter-versions/'||$1::text||'.enc','attached',64,
           decode(repeat('11',32),'hex'),sha256(convert_to('abcdefghij','UTF8')),
           decode(repeat('22',48),'hex'),decode(repeat('33',12),'hex'),
           'content-kek-v1',decode(repeat('44',12),'hex'),
           convert_to(concat('tor.encrypted.chapter.v1',chr(10),$2::text,chr(10),
             $3::text,chr(10),$1::text,chr(10),
             encode(sha256(convert_to('abcdefghij','UTF8')),'hex'),chr(10)),'UTF8'),
           'gzip',now(),now())",
    )
    .bind(&object)
    .bind(paid.chapter)
    .bind(paid.version)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(paid.version)
        .bind(&object)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_reader_manifests
           (chapter_id,version_id,storage_object_id,content_sha256,block_utf16_lengths,
            last_readable_block_index,last_readable_char_offset)
         VALUES($1,$2,$3::uuid,sha256(convert_to('abcdefghij','UTF8')),ARRAY[10],1,10)",
    )
    .bind(paid.chapter)
    .bind(paid.version)
    .bind(&object)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar("SELECT encode(sha256(convert_to('abcdefghij','UTF8')),'hex')")
        .fetch_one(pool)
        .await
        .unwrap()
}
