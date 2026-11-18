//! FND-00d: real MinIO + PG18 bodyless paid V1/V2 under the existing paid_author_pg harness.
use super::*;

#[tokio::test]
#[ignore = "requires disposable PG18 and local isolated MinIO"]
async fn fnd00d_bodyless_writer_recovers_from_s3_outage_and_keeps_paid_v1_after_v2() {
    let url = env::var("TEST_DATABASE_URL").expect("CI isolated database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    const BUCKET_BODYLESS: &str = "tor-private-paid-bodyless-ci";
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "local-paid-bodyless",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(BUCKET_BODYLESS)
        .send()
        .await
        .unwrap();
    let store = S3PrivateStore::new(client, BUCKET_BODYLESS.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = user(&accounts, "fnd00d-bodyless-paid-owner@example.invalid").await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:fnd00d-paid-author','ru','S3-only paid author','public',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let ring = Arc::new(
        ContentKeyring::with_v2(
            Arc::new(ContentKek::from_bytes([0x61; 32])),
            Arc::new(ContentKek::from_bytes([0x62; 32])),
        )
        .unwrap()
        .with_active_id("content-kek-v2")
        .unwrap(),
    );
    let unavailable = PgPaidAuthorPublisher::new(
        catalog.clone(),
        UnavailableStore,
        ring.clone(),
        BUCKET_BODYLESS.to_owned(),
    )
    .unwrap();
    let writer = PgPaidAuthorPublisher::new(
        catalog.clone(),
        store.clone(),
        ring,
        BUCKET_BODYLESS.to_owned(),
    )
    .unwrap();
    let body_v1 = "Paid V1: доступ после проверки S3";
    let command_v1 = generate_session_token();
    let new_chapter = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Private paid bodyless",
        body: body_v1,
        access: "paid",
        price_amount: 250,
        command_key: &command_v1,
    })
    .unwrap();
    assert_eq!(
        unavailable.publish_bodyless(owner, &new_chapter).await,
        Err(AuthorError::Unavailable),
        "a real S3 outage cannot expose the reserved B edition"
    );
    let pending: String = pg(
        &url,
        &format!(
            "SELECT c.visibility || '|' || count(v.id) || '|' ||
                count(v.body_markdown) FROM chapters c
             JOIN chapter_versions v ON v.chapter_id=c.id
             WHERE c.book_id={book} GROUP BY c.visibility"
        ),
    );
    assert_eq!(
        pending, "draft|1|0",
        "failed upload stores zero paid Markdown"
    );
    writer.publish_bodyless(owner, &new_chapter).await.unwrap();
    writer.publish_bodyless(owner, &new_chapter).await.unwrap();
    let current: String = pg(
        &url,
        &format!(
            "SELECT c.id || '|' || p.version_id || '|' || o.kek_id
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id
             JOIN storage_objects o ON o.id=v.storage_object_id
             WHERE c.book_id={book}"
        ),
    );
    let cols = current.split('|').collect::<Vec<_>>();
    assert_eq!(cols.len(), 3);
    let chapter_id: i64 = cols[0].parse().unwrap();
    let v1_id: i64 = cols[1].parse().unwrap();
    assert_eq!(cols[2], "content-kek-v2");
    let v1 = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter_id).unwrap(),
        version_id: PositiveId::new(v1_id).unwrap(),
    };
    let version_port = PgPaidStorage::new(pool.clone());
    let encrypted_v1 = version_port.encrypted_chapter(v1).await.unwrap().unwrap();
    assert!(store.get_verified(&encrypted_v1.object).await.is_ok());
    assert!(!store
        .get_verified(&encrypted_v1.object)
        .await
        .unwrap()
        .windows(body_v1.len())
        .any(|bytes| bytes == body_v1.as_bytes()));

    let body_v2 = "Paid V2: покупатель V1 сохраняет историю";
    let command_v2 = generate_session_token();
    let edit = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(chapter_id as u64),
        number: 1,
        expected_revision: 1,
        title: "Private paid bodyless",
        body: body_v2,
        access: "paid",
        price_amount: 250,
        command_key: &command_v2,
    })
    .unwrap();
    writer.publish_edit_bodyless(owner, &edit).await.unwrap();
    writer.publish_edit_bodyless(owner, &edit).await.unwrap();
    let current_v2: i64 = pg(
        &url,
        &format!("SELECT p.version_id FROM chapter_publications p WHERE p.chapter_id={chapter_id}"),
    )
    .parse()
    .unwrap();
    assert_ne!(v1_id, current_v2);
    let v2 = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter_id).unwrap(),
        version_id: PositiveId::new(current_v2).unwrap(),
    };
    assert!(version_port.encrypted_chapter(v1).await.unwrap().is_some());
    let v2_object = version_port.encrypted_chapter(v2).await.unwrap().unwrap();
    assert_ne!(encrypted_v1.object.object_id, v2_object.object.object_id);
    assert!(store.get_verified(&v2_object.object).await.is_ok());
    let snapshot: String = pg(
        &url,
        &format!(
            "SELECT count(*) || '|' || count(body_markdown) || '|' ||
              count(DISTINCT storage_object_id) FROM chapter_versions
             WHERE chapter_id={chapter_id}"
        ),
    );
    assert_eq!(
        snapshot, "2|0|2",
        "paid V1/V2 must have distinct S3 objects and no Markdown in PG"
    );
    assert_eq!(
        writer.publish_bodyless(owner, &new_chapter).await,
        Err(AuthorError::Conflict),
        "a replay of old V1 must fail closed instead of reverting the current V2"
    );
    let recheck: i64 = pg(
        &url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter_id}"),
    )
    .parse()
    .unwrap();
    assert_eq!(recheck, current_v2);
}
