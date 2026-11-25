//! D-001: logical retention is derived from rights, not from author saves.
//! All fixtures run only in disposable PostgreSQL 18. Immutable S3 editions
//! are exercised separately by S349/FND-00 tests; this test never deletes data.
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_books::{
    repositories::free_s3_publish::PgFreeS3Publication,
    services::authoring::{save_chapter, SaveChapterRequest},
};

async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG18");
    assert!(
        url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"),
        "D-001 must never write a persistent database"
    );
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    PgPool::connect(&url).await.unwrap()
}

async fn user(pool: &PgPool, label: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D001 retention','$argon2id$fixture') RETURNING id",
    )
    .bind(format!(
        "d001-{}-{}@example.invalid",
        label,
        std::process::id()
    ))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn book(pool: &PgPool, owner: i64, label: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D001 retention','public',$2) RETURNING id",
    )
    .bind(format!("ci:d001:{}:{}", label, std::process::id()))
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn counts(pool: &PgPool, chapter: i64) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT logical_editions,reader_retained_editions,physical_materializations
         FROM d001_chapter_logical_counts WHERE chapter_id=$1",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn published(pool: &PgPool, chapter: i64) -> i64 {
    sqlx::query_scalar("SELECT version_id FROM chapter_publications WHERE chapter_id=$1")
        .bind(chapter)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn save(
    book_id: i64,
    chapter: Option<i64>,
    revision: i64,
    text: &str,
    key_char: char,
) -> tor_books::services::authoring::SaveChapter {
    let key = key_char.to_string().repeat(43);
    save_chapter(SaveChapterRequest {
        book_id: book_id as u64,
        chapter_id: chapter.map(|id| id as u64),
        expected_revision: revision,
        number: 1,
        title: "D001 chapter",
        body: text,
        access: "free",
        price_amount: 0,
        command_key: &key,
    })
    .unwrap()
}

fn metadata(text: &str) -> (&'static str, [u8; 32], u64) {
    (
        "tor-private-s349-ci",
        Sha256::digest(text.as_bytes()).into(),
        text.len() as u64,
    )
}

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_ten_unpinned_edits_freeze_only_on_exact_saved_rights() {
    let pool = pool().await;
    let owner = user(&pool, "free-owner").await;
    let saver_one = user(&pool, "free-reader-one").await;
    let saver_two = user(&pool, "free-reader-two").await;
    let book_id = book(&pool, owner, "free-book").await;
    let store = PgFreeS3Publication::new(pool.clone());

    let initial = save(book_id, None, 0, "D001 first body", 'a');
    let first = store.prepare_new(owner, &initial).await.unwrap();
    store
        .attach_verified_and_publish(
            owner,
            book_id,
            (first.chapter_id, first.version_id),
            metadata(&initial.body),
        )
        .await
        .unwrap();
    let chapter = first.chapter_id;
    assert_eq!(counts(&pool, chapter).await, (1, 1, 1));

    let mut current = first.version_id;
    for revision in 1..=10_i64 {
        let text = format!("D001 unpinned revision {revision} 😀");
        let key = char::from(b'a' + revision as u8);
        let request = save(book_id, Some(chapter), revision, &text, key);
        let prepared = store.prepare_edit(owner, &request).await.unwrap();
        assert_eq!(published(&pool, chapter).await, current);
        assert_eq!(counts(&pool, chapter).await.0, 1);
        store
            .attach_edit(owner, &request, &prepared, metadata(&text))
            .await
            .unwrap();
        current = prepared.version_id;
        assert_eq!(published(&pool, chapter).await, current);
        assert_eq!(counts(&pool, chapter).await, (1, 1, revision + 1));
        assert!(store.prepare_edit(owner, &request).await.unwrap().published);
        assert_eq!(counts(&pool, chapter).await, (1, 1, revision + 1));
    }
    // A historical immutable physical identity persists for replay/audit, but
    // is NOT counted as a separate reader-retained logical edition.
    let unpinned: bool = sqlx::query_scalar(
        "SELECT NOT is_logically_retained FROM d001_chapter_materialization_roles
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(first.version_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(unpinned);

    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(saver_one)
    .bind(chapter)
    .bind(current)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(counts(&pool, chapter).await, (1, 1, 11));

    let after_first_pin = save(book_id, Some(chapter), 11, "D001 new head V2", 'm');
    let v2 = store.prepare_edit(owner, &after_first_pin).await.unwrap();
    store
        .attach_edit(
            owner,
            &after_first_pin,
            &v2,
            metadata(&after_first_pin.body),
        )
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (2, 2, 12));
    let frozen: bool = sqlx::query_scalar(
        "SELECT has_saved_free AND is_reader_retained
         FROM d001_chapter_materialization_roles
         WHERE chapter_id=$1 AND version_id=$2",
    )
    .bind(chapter)
    .bind(current)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(frozen);

    let repeated = save(book_id, Some(chapter), 12, "D001 new head edited", 'n');
    let newer = store.prepare_edit(owner, &repeated).await.unwrap();
    store
        .attach_edit(owner, &repeated, &newer, metadata(&repeated.body))
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (2, 2, 13));

    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id)
         VALUES($1,$2,$3)",
    )
    .bind(saver_two)
    .bind(chapter)
    .bind(newer.version_id)
    .execute(&pool)
    .await
    .unwrap();
    let next = save(book_id, Some(chapter), 13, "D001 third logical head", 'o');
    let third = store.prepare_edit(owner, &next).await.unwrap();
    store
        .attach_edit(owner, &next, &third, metadata(&next.body))
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (3, 3, 14));
    assert_eq!(published(&pool, chapter).await, third.version_id);
    pool.close().await;
}

async fn legacy_version(pool: &PgPool, chapter: i64, n: i32, body: &str) -> i64 {
    let sha: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,$2,$3,$4) RETURNING id",
    )
    .bind(chapter)
    .bind(n)
    .bind(body)
    .bind(sha.as_slice())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn buy_fixture(pool: &PgPool, buyer: i64, chapter: i64, version: i64, label: &str) {
    sqlx::query(
        "INSERT INTO chapter_purchase_facts
         (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,$4,100,'RUB')",
    )
    .bind(buyer)
    .bind(chapter)
    .bind(version)
    .bind(format!("d001-settlement-{label}-{}", std::process::id()))
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_purchased_and_layer_archive_pins_are_distinct_from_old_materializations() {
    let pool = pool().await;
    let owner = user(&pool, "paid-owner").await;
    let buyer_one = user(&pool, "paid-buyer-one").await;
    let buyer_two = user(&pool, "paid-buyer-two").await;
    let book_id = book(&pool, owner, "paid-book").await;

    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters
         (book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'D001 paid','public','paid',100,1) RETURNING id",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1 = legacy_version(&pool, chapter, 1, "Paid V1").await;
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (1, 1, 1));

    buy_fixture(&pool, buyer_one, chapter, v1, "first").await;
    assert_eq!(counts(&pool, chapter).await, (1, 1, 1));
    let v2 = legacy_version(&pool, chapter, 2, "Paid V2").await;
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (2, 2, 2));

    // Repeated edits of unpinned V2 do not accumulate reader editions.
    let mut current = v2;
    for n in 3..=8_i32 {
        let fresh = legacy_version(&pool, chapter, n, &format!("Paid physical {n}")).await;
        sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
            .bind(chapter)
            .bind(fresh)
            .execute(&pool)
            .await
            .unwrap();
        current = fresh;
        assert_eq!(counts(&pool, chapter).await, (2, 2, i64::from(n)));
    }
    buy_fixture(&pool, buyer_two, chapter, current, "second").await;
    let next = legacy_version(&pool, chapter, 9, "Paid logical V3").await;
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(next)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(counts(&pool, chapter).await, (3, 3, 9));

    // Reader cursor and saved command are *not* paid rights. The two exact
    // direct purchase facts are enough to preserve the original digests.
    let purchased: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM d001_chapter_materialization_roles
         WHERE chapter_id=$1 AND has_direct_purchase",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(purchased, 2);

    // A historical FREE Layer archive is an exact version-specific pin,
    // unlike paid Layer terms that resolve a currently published chapter.
    let free: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'D001 free','public','free',0) RETURNING id",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let free_v1 = legacy_version(&pool, free, 1, "Free archived V1").await;
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(free)
        .bind(free_v1)
        .execute(&pool)
        .await
        .unwrap();
    let free_v2 = legacy_version(&pool, free, 2, "Free latest V2").await;
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(free)
        .bind(free_v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(counts(&pool, free).await, (1, 1, 2));
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions
         (book_id,request_revision,chapter_id,version_id) VALUES($1,1,$2,$3)",
    )
    .bind(book_id)
    .bind(free)
    .bind(free_v1)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(counts(&pool, free).await, (2, 2, 2));
    pool.close().await;
}

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_s3_only_reversion_preserves_fresh_physical_identity_and_old_receipts() {
    let pool = pool().await;
    let owner = user(&pool, "revert-owner").await;
    let book_id = book(&pool, owner, "revert-book").await;
    let store = PgFreeS3Publication::new(pool.clone());

    let initial = save(book_id, None, 0, "Original text A", 'p');
    let first = store.prepare_new(owner, &initial).await.unwrap();
    store
        .attach_verified_and_publish(
            owner,
            book_id,
            (first.chapter_id, first.version_id),
            metadata(&initial.body),
        )
        .await
        .unwrap();

    let to_b = save(book_id, Some(first.chapter_id), 1, "Changed text B", 'q');
    let second = store.prepare_edit(owner, &to_b).await.unwrap();
    store
        .attach_edit(owner, &to_b, &second, metadata(&to_b.body))
        .await
        .unwrap();

    // Returning to identical bytes A must NOT reuse version_id V1:
    // immutable S3 keys, SHA receipts, command replay and future pins
    // must continue to refer to their original physical materializations.
    let to_a = save(book_id, Some(first.chapter_id), 2, "Original text A", 'r');
    let third = store.prepare_edit(owner, &to_a).await.unwrap();
    assert_ne!(third.version_id, first.version_id);
    assert_ne!(third.version_id, second.version_id);
    assert_eq!(published(&pool, first.chapter_id).await, second.version_id);
    store
        .attach_edit(owner, &to_a, &third, metadata(&to_a.body))
        .await
        .unwrap();
    assert_eq!(published(&pool, first.chapter_id).await, third.version_id);
    assert_eq!(counts(&pool, first.chapter_id).await, (1, 1, 3));

    let original_sha: [u8; 32] = Sha256::digest(initial.body.as_bytes()).into();
    let receipts: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*)::bigint,
                count(*) FILTER (WHERE v.content_sha256=$2)::bigint,
                count(DISTINCT f.object_key)::bigint
         FROM chapter_versions v
         JOIN free_chapter_storage_objects f
           ON f.chapter_id=v.chapter_id AND f.version_id=v.id
         WHERE v.chapter_id=$1",
    )
    .bind(first.chapter_id)
    .bind(original_sha.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, (3, 2, 3));

    let replay = store.prepare_edit(owner, &to_a).await.unwrap();
    assert!(replay.published);
    assert_eq!(replay.version_id, third.version_id);
    assert_eq!(counts(&pool, first.chapter_id).await, (1, 1, 3));
    pool.close().await;
}

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_paid_bodyless_equal_hash_is_not_a_unique_version_conflict() {
    let pool = pool().await;
    let owner = user(&pool, "paid-revert-owner").await;
    let book_id = book(&pool, owner, "paid-revert-book").await;
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters
         (book_id,chapter_no,title,access_level,price_amount,visibility)
         VALUES($1,1,'Paid revert','paid',100,'draft') RETURNING id",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let hash: [u8; 32] = Sha256::digest(b"Identical paid bytes A").into();
    for number in 1..=2_i32 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions
             (chapter_id,version_no,body_markdown,content_sha256)
             VALUES($1,$2,NULL,$3) RETURNING id",
        )
        .bind(chapter)
        .bind(number)
        .bind(hash.as_slice())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(id > 0);
    }
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_versions
         WHERE chapter_id=$1 AND content_sha256=$2 AND body_markdown IS NULL",
    )
    .bind(chapter)
    .bind(hash.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows, 2,
        "each paid AES-GCM AAD binds its own physical version"
    );
    let legacy: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,3,'Identical paid bytes A',$2) RETURNING id",
    )
    .bind(chapter)
    .bind(hash.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        legacy > 0,
        "legacy text and bodyless S3 versions must coexist"
    );
    let legacy_duplicate = sqlx::query(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,4,'Identical paid bytes A',$2)",
    )
    .bind(chapter)
    .bind(hash.as_slice())
    .execute(&pool)
    .await;
    assert!(
        legacy_duplicate.is_err(),
        "legacy PG bodies stay hash-unique"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "disposable PG18 tor_ci + GitHub CI only"]
async fn d001_inventory_reports_installed_chapter_version_fk_obligations() {
    use std::io::Write;

    let pool = pool().await;
    let rows: Vec<(
        String,
        String,
        String,
        Vec<String>,
        Vec<String>,
        bool,
        String,
        String,
    )> = sqlx::query_as(include_str!("d001_reference_inventory.sql"))
        .fetch_all(&pool)
        .await
        .unwrap();
    let mut log = std::io::stdout().lock();
    for (schema, table, name, source, target, valid, on_delete, class) in &rows {
        writeln!(
            log,
            "D001_VERSION_FK {schema}.{table} {name} {source:?} -> {target:?} \
             validated={valid} delete={on_delete} class={class}"
        )
        .unwrap();
    }
    for (want_table, want_class) in [
        ("chapter_purchase_facts", "purchase-pin"),
        ("reader_saved_free_chapters", "saved-free-pin"),
        ("book_layer_free_archive_versions", "layer-free-pin"),
        ("reader_free_opened_editions", "verified-first-open"),
        ("reader_free_open_intents", "delivery-intent"),
        ("chapter_purchase_commands", "checkout-replay"),
    ] {
        assert!(
            rows.iter().any(|(_, table, _, _, _, _, _, class)| {
                table == want_table && class == want_class
            }),
            "missing installed constraint for {want_table}: {want_class}"
        );
    }
    let unreviewed = rows.iter().filter(|row| row.7 == "unreviewed").count();
    // Deliberately no clearance decision: unknown and soft references are not
    // covered, and no schema inventory is a deletion, ACL or retention gate.
    writeln!(
        log,
        "D001_VERSION_FK_SCHEMA_REPORT total={} unreviewed={unreviewed} soft_refs=out_of_scope",
        rows.len()
    )
    .unwrap();
}
