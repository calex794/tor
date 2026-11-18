//! FND-00g: real owner HTTP editor opens only exact current paid S3-only
//! source, then saves immutable V2 without PostgreSQL body Markdown.
use super::*;

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn fnd00_paid_bodyless_owner_http_get_edit_v2_retains_purchased_v1() {
    let url = env::var("TEST_DATABASE_URL").expect("CI isolated database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));

    const BUCKET_BODYLESS_HTTP: &str = "tor-private-paid-bodyless-http-ci";
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "local-paid-http-bodyless",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(BUCKET_BODYLESS_HTTP)
        .send()
        .await
        .unwrap();
    let store = S3PrivateStore::new(client, BUCKET_BODYLESS_HTTP.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, session) = user(&accounts, "fnd00-http-owner@example.invalid").await;
    let (buyer, buyer_session) = user(&accounts, "fnd00-http-buyer@example.invalid").await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:fnd00-paid-http-owner','ru','Paid S3 editor','public',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let ring = Arc::new(
        ContentKeyring::with_v2(
            Arc::new(ContentKek::from_bytes([0x31; 32])),
            Arc::new(ContentKek::from_bytes([0x32; 32])),
        )
        .unwrap()
        .with_active_id("content-kek-v2")
        .unwrap(),
    );
    let writer = PgPaidAuthorPublisher::new(
        catalog.clone(),
        store.clone(),
        ring.clone(),
        BUCKET_BODYLESS_HTTP.to_owned(),
    )
    .unwrap();
    let initial_body = "Paid bodyless V1 source";
    let initial_key = generate_session_token();
    let v1_request = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "Immutable paid editor",
        body: initial_body,
        access: "paid",
        price_amount: 225,
        command_key: &initial_key,
    })
    .unwrap();
    writer.publish_bodyless(owner, &v1_request).await.unwrap();
    let published = pg(
        &url,
        &format!(
            "SELECT c.id || '|' || p.version_id
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.book_id={book}"
        ),
    );
    let parts = published.split('|').collect::<Vec<_>>();
    assert_eq!(parts.len(), 2);
    let chapter: i64 = parts[0].parse().unwrap();
    let v1: i64 = parts[1].parse().unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_purchase_facts
             (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
             VALUES({buyer},{chapter},{v1},'fnd00-http-v1-{chapter}',225,'RUB')"
        ),
    );

    let signer: Arc<dyn PrivateObjectDeliveryPort> = Arc::new(
        PgPrivateDelivery::with_keyring(
            pool.clone(),
            store.clone(),
            ring,
            BUCKET_BODYLESS_HTTP.to_owned(),
        )
        .unwrap(),
    );
    let app = tor_web::router_with_postgres_reading_auth_paid_grants_browser_author(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        (false, ORIGIN.to_owned()),
        (signer, ENDPOINT.to_owned()),
        None,
        writer,
    )
    .unwrap();
    let csrf = generate_session_token();
    let edit_path = format!("/me/books/{book}/chapters/{chapter}/edit");
    let (denied, denied_page) = browse(&app, &edit_path, &buyer_session, &csrf).await;
    assert_eq!(denied, StatusCode::NOT_FOUND);
    assert!(!denied_page.contains(initial_body));
    let (status, page) = browse(&app, &edit_path, &session, &csrf).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains(initial_body));
    assert!(page.contains("Immutable paid editor"));

    let command = generate_session_token();
    let post = format!(
        "number=999&title=Tampered&body=Paid%20bodyless%20V2%20source&expected_revision=1&command_key={command}&csrf_token={csrf}"
    );
    for expected in [StatusCode::SEE_OTHER, StatusCode::SEE_OTHER] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&edit_path)
                    .method("POST")
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, cookie(&session, &csrf))
                    .body(Body::from(post.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let current_v2: i64 = pg(
        &url,
        &format!(
            "SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"
        ),
    )
    .parse()
    .unwrap();
    assert_ne!(v1, current_v2);
    let snapshot = pg(
        &url,
        &format!(
            "SELECT count(*) || '|' || count(body_markdown) || '|' ||
                    count(DISTINCT storage_object_id)
             FROM chapter_versions WHERE chapter_id={chapter}"
        ),
    );
    assert_eq!(snapshot, "2|0|2");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT title || '|' || chapter_no || '|' || edit_revision
                 FROM chapters WHERE id={chapter}"
            ),
        ),
        "Immutable paid editor|1|2",
        "form fields must not change immutable paid metadata"
    );
    let first = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(v1).unwrap(),
    };
    assert!(PgPaidStorage::new(pool).encrypted_chapter(first).await.unwrap().is_some());
    let (status, page) = browse(&app, &edit_path, &session, &csrf).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Paid bodyless V2 source"));
    assert!(!page.contains(initial_body));
}
