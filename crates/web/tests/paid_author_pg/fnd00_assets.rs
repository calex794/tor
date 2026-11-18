//! FND-00: a paid image is pinned to an S3-only draft without PG Markdown.
use super::*;
use tor_contracts::paid_delivery::ObjectId;

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn fnd00_paid_image_upload_never_creates_postgres_markdown() {
    let url = env::var("TEST_DATABASE_URL").expect("CI isolated database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));

    const BUCKET_BODYLESS_ASSETS: &str = "tor-private-paid-assets-bodyless-ci";
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "local-paid-bodyless-assets",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(BUCKET_BODYLESS_ASSETS)
        .send()
        .await
        .unwrap();
    let store = S3PrivateStore::new(client, BUCKET_BODYLESS_ASSETS.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = user(&accounts, "fnd00-assets-bodyless-owner@example.invalid").await;
    let (other, _) = user(&accounts, "fnd00-assets-bodyless-other@example.invalid").await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:fnd00-assets-bodyless','ru','Bodyless illustrations','public',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let object_id = ObjectId::parse("f00da550-0001-4000-8000-000000000001").unwrap();
    let body = format!("Paid body without PG text\n\n![Illustration](asset:{object_id})");
    let command = generate_session_token();
    let request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Private illustrated V1",
        body: &body,
        access: "paid",
        price_amount: 145,
        command_key: &command,
    })
    .unwrap();

    let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([255, 0, 0, 255]),
    ));
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let png = png.into_inner();
    let ring = Arc::new(ContentKeyring::v1_only(Arc::new(ContentKek::from_bytes(
        [0x6f; 32],
    ))));
    let uploader =
        PgPaidAuthorAssets::new(catalog, store, ring, BUCKET_BODYLESS_ASSETS.to_owned()).unwrap();

    assert!(matches!(
        uploader
            .upload_to_draft_with_id_bodyless(other, &request, 1, &png, "image/png", &object_id)
            .await,
        Err(AuthorError::Missing)
    ));
    assert_eq!(
        uploader
            .upload_to_draft_with_id_bodyless(owner, &request, 1, &png, "image/png", &object_id)
            .await,
        Ok(object_id.clone()),
        "a verified private image must attach to its exact bodyless draft"
    );
    assert_eq!(
        uploader
            .upload_to_draft_with_id_bodyless(owner, &request, 1, &png, "image/png", &object_id)
            .await,
        Ok(object_id.clone()),
        "same command, version and image must be idempotent"
    );

    let row = pg(
        &url,
        &format!(
            "SELECT c.visibility || '|' || count(v.id) || '|' ||
                    count(v.body_markdown) || '|' || count(a.storage_object_id)
             FROM chapters c
             JOIN chapter_versions v ON v.chapter_id=c.id
             LEFT JOIN chapter_version_assets a
               ON a.chapter_id=c.id AND a.version_id=v.id
             WHERE c.book_id={book} GROUP BY c.visibility"
        ),
    );
    assert_eq!(
        row, "draft|1|0|1",
        "private image staging cannot fall back to PostgreSQL Markdown"
    );
    assert!(
        matches!(
            PgPaidStorage::new(pool)
                .prepare_new_paid_chapter(owner, &request)
                .await,
            Err(AuthorError::Conflict)
        ),
        "a legacy writer must not attach Markdown to a bodyless draft"
    );
}