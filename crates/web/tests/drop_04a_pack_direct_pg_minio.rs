//! DROP-04a: real private MinIO + PG18 direct PACK staged receipt.
//! Source goes directly from the approved import text to one seekable PACK;
//! NO free_chapter_storage_objects, publication, reader or per-chapter PUT.
#![cfg(feature = "s3-free")]
use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use sha2::{Digest, Sha256};
use std::{env, process::Command};
use tor_books::{
    repositories::{free_s3_publish::PgFreeS3Publication, PgCatalog},
    services::{
        authoring::AuthorError,
        chapter_file_import::{fingerprint, parse_text},
    },
};
use tor_imports::{
    domain::ChapterImportClaim,
    repositories::postgres::PgImportJobs,
};
use tor_storage::{
    repositories::{
        free_chapter_s3::S3FreeChapterStore,
        free_pack_proof_s3::S3PackProofStore,
        free_pack_write_s3::S3FreePackWriter,
    },
    services::free_chapter::FreeChapterObject,
};
use tor_web::free_author_publish::PgFreeAuthorPublisher;

const BUCKET: &str = "tor-drop-direct-stage-ci";
const ENDPOINT: &str = "http://127.0.0.1:9000";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_ci"));
    let output=Command::new("psql").args([
        "-X","-A","-t","-q","-v","ON_ERROR_STOP=1","--dbname",url,
        "--command",sql
    ]).output().unwrap();
    assert!(output.status.success(), "PG fixture failed: {}",
            String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn s3() -> Client {
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").unwrap(), ENDPOINT);
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                None,None,"drop-04a-test"
            ))
            .endpoint_url(ENDPOINT).force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build()
    )
}

#[tokio::test(flavor = "multi_thread",worker_threads=2)]
#[ignore = "requires disposable PostgreSQL18 + MinIO, never production"]
async fn drop_04a_stages_direct_pack_without_any_single_and_fences_stale_epoch() {
    let url=env::var("TEST_DATABASE_URL").unwrap();
    let client=s3();
    let _=client.create_bucket().bucket(BUCKET).send().await;
    let pool=PgCatalog::connect(&url).await.unwrap().pool().clone();
    let actor: i64=pg(&url,
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('drop-04a-ci@tor.example','DROP direct CI','$argon2id$test')
         RETURNING id"
    ).parse().unwrap();
    let book: i64=pg(&url,&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:drop:04a','ru','DROP direct','draft',{actor})
         RETURNING id"
    )).parse().unwrap();
    let source="# Глава 1: Начало\nОдин текст 😀\n\n# Глава 2: Далее\nВторой текст 中文";
    let chapters=parse_text("direct-chapters.txt",source).unwrap();
    assert_eq!(chapters.len(),2);
    let key="d".repeat(43);
    let input_key=format!("staging/chapters/{actor}/{book}/{key}");
    let hash: [u8;32]=Sha256::digest(source.as_bytes()).into();
    client.put_object().bucket(BUCKET).key(&input_key)
        .body(ByteStream::from(source.as_bytes().to_vec()))
        .send().await.unwrap();
    let read=client.get_object().bucket(BUCKET).key(&input_key)
        .send().await.unwrap().body.collect().await.unwrap().into_bytes();
    assert_eq!(Sha256::digest(&read).as_slice(),hash.as_slice());
    let digest=fingerprint(book,&chapters);
    let jobs=PgImportJobs::new(pool.clone());
    let job=jobs.enqueue_verified(actor,book,&key,&input_key,
        "direct-chapters.txt",&hash,source.len() as i64,
        &digest,chapters.len() as i32).await.unwrap();
    pg(&url,&format!(
        "UPDATE chapter_import_jobs
         SET status='running',attempts=1,lease_epoch=1,
             locked_until=clock_timestamp()+interval '2 minutes'
         WHERE id={job}"
    ));
    let claim=ChapterImportClaim {
        job_id:job,parent_trace_id:None,book_id:book,actor_id:actor,
        command_key:key,source_key:input_key,
        source_filename:"direct-chapters.txt".into(),
        source_sha256:hash.to_vec(),source_bytes:source.len() as i64,
        approved_fingerprint:digest.to_vec(),
        expected_chapter_count:chapters.len() as i32,attempts:1,epoch:1,
    };
    let store=S3FreeChapterStore::new(client.clone(),BUCKET.into()).unwrap();
    let author=PgFreeAuthorPublisher::new(
        PgFreeS3Publication::new(pool.clone()),store,BUCKET.into()
    ).unwrap();
    let writer=S3FreePackWriter::new(client.clone());
    let first=author.stage_direct_pack_only(&claim,&chapters,&writer)
        .await.unwrap();
    assert_eq!(first.frames.len(),2);
    let second=author.stage_direct_pack_only(&claim,&chapters,&writer)
        .await.unwrap();
    assert_eq!(first,second, "lost commit ACK cannot rewrite the immutable PACK");
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM free_pack_direct_import_stages WHERE job_id={job}"
    )), "1");
    let candidates: i64=pg(&url,&format!(
        "SELECT count(*) FROM free_chapter_storage_objects f
          JOIN free_chapter_import_reservations r ON r.version_id=f.version_id
          WHERE r.book_id={book}"
    )).parse().unwrap();
    assert_eq!(candidates,0,"no per-edition SINGLE upload was performed");
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id={book}"
    )), "0");
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM chapters WHERE book_id={book}"
    )), "0","all identities remain reserved and invisible");
    let expected: Vec<_>=first.frames.iter().map(|f|
        FreeChapterObject::from_metadata(
            f.chapter_id,f.version_id,f.content_sha256,f.size_bytes
        ).unwrap()
    ).collect();
    let verified=S3PackProofStore::new(client.clone())
        .verify_independent_full_get(&tor_storage::services::free_pack_proof::PackProofTarget {
            bucket:first.target.bucket.clone(),
            pack_key:first.target.pack_key.clone(),
            manifest_key:first.target.manifest_key.clone(),
            pack_sha256:first.target.pack_sha256,
            manifest_sha256:first.target.manifest_sha256,
            pack_bytes:first.target.pack_bytes,
            manifest_bytes:first.target.manifest_bytes,
            seek_table_start:first.target.seek_table_start,
        },&expected).await.unwrap();
    assert_eq!(verified.placement_receipt(),first);
    // Exact job source fingerprint is B authority, not a client descriptor.
    let mut forged=claim.clone();
    forged.approved_fingerprint[0]^=1;
    assert!(matches!(
        author.stage_direct_pack_only(&forged,&chapters,&writer).await,
        Err(AuthorError::Conflict)
    ));
    // Late worker with an expired/stolen epoch cannot stage or publish anything.
    pg(&url,&format!(
        "UPDATE chapter_import_jobs SET lease_epoch=lease_epoch+1,
         locked_until=clock_timestamp()+interval '2 minutes' WHERE id={job}"
    ));
    assert!(matches!(
        author.stage_direct_pack_only(&claim,&chapters,&writer).await,
        Err(AuthorError::Conflict)
    ));
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM chapter_publications p
         JOIN chapters c ON c.id=p.chapter_id WHERE c.book_id={book}"
    )), "0");
    pool.close().await;
}
