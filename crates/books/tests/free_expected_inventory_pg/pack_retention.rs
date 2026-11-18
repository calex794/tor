use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{path::Path, process::Command};

fn export(url: &str, pack_id: i64) -> std::process::Output {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/drop07_export_pack_retention.sql");
    Command::new("psql")
        .args(["-Xq", "--dbname", url, "-v", &format!("pack_id={pack_id}")])
        .arg("-f")
        .arg(script)
        .output()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated disposable PostgreSQL18 /tor_ci"]
async fn drop_07_pack_export_preserves_known_references_and_leaves_uncertain_members_unknown() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let (actor, book): (i64, i64) = sqlx::query_as(
        "WITH actor AS (
             INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'DROP07 PACK retention','$argon2id$test') RETURNING id
         )
         INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         SELECT $2,'ru','DROP07 PACK retention','draft',id FROM actor
         RETURNING owner_user_id,id",
    )
    .bind(format!("drop07pack-{nonce}@example.invalid"))
    .bind(format!("ci:drop07pack:{nonce}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let editions: Vec<(i64, i64)> = sqlx::query_as(
        "WITH fresh_chapters AS (
             INSERT INTO chapters(book_id,chapter_no,title,admin_hold)
             SELECT $1,n,'FREE fixture',n=6 FROM generate_series(1,6) AS n
             RETURNING id,chapter_no
         ), versions AS (
             INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
             SELECT c.id,n,'fixture' || n,sha256(convert_to('fixture' || n,'UTF8'))
             FROM fresh_chapters c
             CROSS JOIN LATERAL generate_series(1,CASE WHEN c.chapter_no=1 THEN 2 ELSE 1 END) AS n
             RETURNING chapter_id,id,version_no
         )
         SELECT v.chapter_id,v.id FROM versions v JOIN fresh_chapters c ON c.id=v.chapter_id
         ORDER BY c.chapter_no,v.version_no",
    )
    .bind(book)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(editions.len(), 7);
    let pack_id: i64 = sqlx::query_scalar(
        "INSERT INTO free_storage_packs(bucket,object_key,manifest_key,pack_sha256,manifest_sha256,
                                        manifest_bytes,pack_bytes,seek_table_start)
         VALUES ('tor-drop07pack',$1,$2,decode(repeat('cd',32),'hex'),
                 decode(repeat('ef',32),'hex'),200,1024,999) RETURNING id",
    )
    .bind(format!("private/free-packs/ci/drop07-retention-{book}.zst"))
    .bind(format!(
        "private/free-packs/ci/drop07-retention-{book}.manifest.json"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !export(&url, pack_id).status.success(),
        "PACK without frames"
    );

    for (index, &(chapter, version)) in editions.iter().enumerate() {
        sqlx::query(
            "INSERT INTO free_edition_storage_generations
                 (chapter_id,version_id,generation,placement_kind,content_sha256,
                  size_bytes,pack_id,frame_offset,frame_bytes)
             SELECT chapter_id,id,1,'pack',content_sha256,octet_length(body_markdown),$3,$4,10
             FROM chapter_versions WHERE chapter_id=$1 AND id=$2",
        )
        .bind(chapter)
        .bind(version)
        .bind(pack_id)
        .bind((index * 10) as i64)
        .execute(&pool)
        .await
        .unwrap();
    }

    let (unknown, current, saved, archived, active, pending) = (
        editions[0],
        editions[1],
        editions[2],
        editions[3],
        editions[4],
        editions[5],
    );
    assert_eq!(unknown.0, current.0);
    assert_ne!(unknown.1, current.1);
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(current.0)
        .bind(current.1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_saved_free_chapters(user_id,chapter_id,version_id) VALUES($1,$2,$3)",
    )
    .bind(actor)
    .bind(saved.0)
    .bind(saved.1)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions(book_id,request_revision,chapter_id,version_id)
         VALUES($1,1,$2,$3)",
    )
    .bind(book)
    .bind(archived.0)
    .bind(archived.1)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT promote_free_edition_storage($1,$2,0,1)")
            .bind(active.0)
            .bind(active.1)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    sqlx::query(
        "WITH job AS (
             INSERT INTO free_pack_jobs(actor_user_id,book_id,approved_fingerprint,expected_chapter_count)
             VALUES($1,$2,decode(repeat('13',32),'hex'),1) RETURNING id
         )
         INSERT INTO free_pack_job_members(job_id,chapter_id,version_id) SELECT id,$3,$4 FROM job",
    )
    .bind(actor)
    .bind(book)
    .bind(pending.0)
    .bind(pending.1)
    .execute(&pool)
    .await
    .unwrap();

    // Check every frame and reason. The source fingerprint and offset make
    // the report comparable to the independently verified PACK and manifest.
    let result = export(&url, pack_id);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    let mut lines = output.lines();
    assert!(lines
        .next()
        .unwrap()
        .ends_with("pending_pack_work,retention"));
    let expected = [
        (unknown, "f,f,f,f,f,f,UNKNOWN"),
        (current, "f,t,f,f,f,f,RETAIN"),
        (saved, "f,f,t,f,f,f,RETAIN"),
        (archived, "f,f,f,t,f,f,RETAIN"),
        (active, "t,f,f,f,f,f,RETAIN"),
        (pending, "f,f,f,f,f,t,RETAIN"),
        (editions[6], "f,f,f,f,t,f,RETAIN"),
    ];
    for (index, ((chapter, version), reasons)) in expected.iter().enumerate() {
        let body = if index == 1 { b"fixture2" } else { b"fixture1" };
        assert_eq!(
            lines.next().unwrap(),
            format!(
                "{pack_id},{},{},{},10,{chapter},{version},1,8,{},{reasons}",
                "cd".repeat(32),
                "ef".repeat(32),
                index * 10,
                hex::encode(Sha256::digest(body)),
            )
        );
    }
    assert!(lines.next().is_none());
    assert!(!export(&url, 0).status.success(), "unregistered PACK");

    sqlx::query(
        "INSERT INTO free_edition_storage_generations
             (chapter_id,version_id,generation,placement_kind,content_sha256,
              size_bytes,pack_id,frame_offset,frame_bytes)
         SELECT chapter_id,version_id,2,placement_kind,content_sha256,
                size_bytes,pack_id,70,frame_bytes
         FROM free_edition_storage_generations
         WHERE pack_id=$1 AND frame_offset=0",
    )
    .bind(pack_id)
    .execute(&pool)
    .await
    .unwrap();
    let ambiguous = export(&url, pack_id);
    assert!(
        !ambiguous.status.success(),
        "duplicate edition within the PACK"
    );
    assert!(
        ambiguous.stdout.is_empty(),
        "no partial CSV for ambiguous membership"
    );
}
