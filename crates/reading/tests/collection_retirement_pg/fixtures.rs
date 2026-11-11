use super::*;

#[derive(Clone, Copy)]
pub struct Book {
    pub id: i64,
    pub chapter: i64,
    pub version: i64,
}

pub async fn pool(max: u32) -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("current disposable PostgreSQL URL");
    let options = PgConnectOptions::from_str(&url).unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_port(), 5432);
    assert_eq!(options.get_database(), Some("tor_ci"));
    assert_eq!(options.get_username(), "tor_app");
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .connect_with(options)
        .await
        .unwrap();
    let installed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version=202611168903 AND success)
         AND EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid='rec_subjects'::regclass
           AND tgname='retired_collection_write_fence' AND tgenabled='O')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        installed,
        "this business proof requires the actual current retirement barrier"
    );
    // These approved projections must remain legal row-locking ports, even
    // though retired analytics identities are no longer exposed through them.
    for view in ["reader_activity_consent", "reader_flow_consent"] {
        let rows = sqlx::query(&format!("SELECT user_id FROM {view} FOR UPDATE"))
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(rows.is_empty());
    }
    pool
}

pub async fn actor(pool: &PgPool, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('retired-c-'||$1||'-'||gen_random_uuid()||'@example.invalid',
           'Current C fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn book(pool: &PgPool, paid: bool) -> Book {
    let id = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES('ci:retired-c:'||gen_random_uuid(),'ru','Current C fixture','public','basic')
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

pub fn legacy_sync(pool: &PgPool) -> PgReadingSync {
    PgReadingSync::new(pool.clone())
        .with_activity_collection(true)
        .with_book_return_collection(true)
        .with_flow_collection(true)
}

pub async fn optional_counts(pool: &PgPool) -> Vec<i64> {
    let mut values = Vec::new();
    for table in [
        "rec_subjects",
        "reader_flow_book_actions",
        "reader_flow_activity_days",
        "reader_flow_read_receipts",
        "reader_activity_days",
        "reader_return_subjects",
        "reader_book_return_subjects",
    ] {
        values.push(
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    values
}

pub async fn assert_no_optional(pool: &PgPool, reader: i64, baseline: &[i64]) {
    assert_eq!(
        optional_counts(pool).await,
        baseline,
        "canonical business commands must not create optional subjects or raw derivatives"
    );
    let records: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM rec_subjects WHERE user_id=$1)
          +(SELECT count(*) FROM event_deliveries d JOIN outbox_events e ON e.id=d.event_id
            WHERE e.payload->>'user_id'=$1::text AND d.consumer IN
              ('recommendation-feedback-v1','flow-read-attribution-v1'))",
    )
    .bind(reader)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        records, 0,
        "retired consumers receive no fabricated delivery receipts"
    );
}

pub async fn business_actions(pool: &PgPool, reader: i64, book: i64) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM reader_library_items WHERE user_id=$1 AND book_id=$2),
          (SELECT count(*) FROM reader_book_follows WHERE user_id=$1 AND book_id=$2),
          (SELECT p.version_id FROM reader_saved_free_chapters p
             JOIN chapters c ON c.id=p.chapter_id WHERE p.user_id=$1 AND c.book_id=$2)",
    )
    .bind(reader)
    .bind(book)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn canonical_counts(pool: &PgPool, reader: i64) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM reader_chapter_starts WHERE user_id=$1),
          (SELECT count(*) FROM reader_chapter_completions WHERE user_id=$1)",
    )
    .bind(reader)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn canonical_snapshot(pool: &PgPool, reader: i64) -> String {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'starts',(SELECT jsonb_agg(to_jsonb(s) ORDER BY chapter_id) FROM reader_chapter_starts s WHERE user_id=$1),
          'completions',(SELECT jsonb_agg(to_jsonb(c) ORDER BY chapter_id) FROM reader_chapter_completions c WHERE user_id=$1),
          'events',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM outbox_events e
             WHERE payload->>'user_id'=$1::text AND event_type IN('ChapterStartedV1','ChapterCompletedV1'))
        )::text",
    )
    .bind(reader)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn assert_canonical(pool: &PgPool, reader: i64, book: Book, purchase: Option<i64>) {
    let verified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
           SELECT first_version_id,purchase_id,source_event_id,'ChapterStartedV1' AS kind
             FROM reader_chapter_starts WHERE user_id=$1 AND book_id=$2 AND chapter_id=$3
           UNION ALL
           SELECT first_version_id,purchase_id,source_event_id,'ChapterCompletedV1'
             FROM reader_chapter_completions WHERE user_id=$1 AND book_id=$2 AND chapter_id=$3
         ) c JOIN outbox_events e ON e.id=c.source_event_id AND e.event_type=c.kind
         WHERE c.first_version_id=$4 AND c.purchase_id IS NOT DISTINCT FROM $5
           AND e.payload->>'user_id'=$1::text AND e.payload->>'book_id'=$2::text
           AND e.payload->>'chapter_id'=$3::text AND e.payload->>'version_id'=$4::text
           AND NOT (e.required_consumers && ARRAY['recommendation-feedback-v1','flow-read-attribution-v1'])
           AND ((c.kind='ChapterStartedV1' AND cardinality(e.required_consumers)=0
                 AND e.delivered_at IS NOT NULL)
             OR (c.kind='ChapterCompletedV1'
                 AND e.required_consumers=ARRAY['ranking-eligibility-v1','achievements-v1']))",
    )
    .bind(reader)
    .bind(book.id)
    .bind(book.chapter)
    .bind(book.version)
    .bind(purchase)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        verified, 2,
        "canonical source identity, version, payload and business obligations survive"
    );
}

pub async fn purchase(pool: &PgPool, reader: i64, book: Book) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
         (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,'ci:retired-c:'||gen_random_uuid(),100,'RUB') RETURNING id",
    )
    .bind(reader)
    .bind(book.chapter)
    .bind(book.version)
    .fetch_one(pool)
    .await
    .unwrap()
}

pub async fn encrypted(pool: &PgPool, book: Book) -> String {
    let object: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO storage_objects(id,object_kind,bucket,object_key,state,size_bytes,
           sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,kek_id,
           object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES($1::uuid,'chapter','tor-private-retired-ci',
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
    .bind(book.chapter)
    .bind(book.version)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(book.version)
        .bind(&object)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_reader_manifests(chapter_id,version_id,storage_object_id,
           content_sha256,block_utf16_lengths,last_readable_block_index,last_readable_char_offset)
         VALUES($1,$2,$3::uuid,sha256(convert_to('abcdefghij','UTF8')),ARRAY[10],1,10)",
    )
    .bind(book.chapter)
    .bind(book.version)
    .bind(object)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar("SELECT encode(sha256(convert_to('abcdefghij','UTF8')),'hex')")
        .fetch_one(pool)
        .await
        .unwrap()
}
