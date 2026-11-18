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
    .unwrap()
    .with_bodyless_http();
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
    // Author HTTP creates new paid V1 directly in private S3 when the
    // explicitly selected bodyless mode is active. Same command is idempotent.
    let http_command = generate_session_token();
    let http_form = format!(
        "number=2&title=Browser%20paid&body=Browser%20bodyless%20V1&access=paid&price_amount=100&command_key={http_command}&csrf_token={csrf}"
    );
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/me/books/{book}/chapters"))
                    .method("POST")
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, cookie(&session, &csrf))
                    .body(Body::from(http_form.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    let http_body = pg(
        &url,
        &format!(
            "SELECT count(v.id) || '|' || count(v.body_markdown)
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id
             WHERE c.book_id={book} AND c.chapter_no=2"
        ),
    );
    assert_eq!(
        http_body, "1|0",
        "browser-created V1 must have no PG plaintext"
    );

    // Browser illustrated paid V1 must reserve the same bodyless B version,
    // verify G's image GET, and publish without any PG Markdown fallback.
    let illustrated_key = generate_session_token();
    let png_image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([16, 48, 96, 255]),
    ));
    let mut png_cursor = Cursor::new(Vec::new());
    png_image
        .write_to(&mut png_cursor, image::ImageFormat::Png)
        .unwrap();
    let png = png_cursor.into_inner();
    let boundary = "fnd00-bodyless-illustration-http";
    let mut payload = Vec::new();
    for (name, value) in [
        ("csrf_token", csrf.as_str()),
        ("command_key", illustrated_key.as_str()),
        ("number", "3"),
        ("title", "Bodyless image"),
        ("price_amount", "175"),
        ("body", "Paid V1 with a sealed image"),
    ] {
        payload.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    payload.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"illustration\"; filename=\"img.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    payload.extend_from_slice(&png);
    payload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    for (who, origin, expected) in [
        (buyer_session.as_str(), ORIGIN, StatusCode::NOT_FOUND),
        (
            session.as_str(),
            "https://wrong-origin.invalid",
            StatusCode::FORBIDDEN,
        ),
        (session.as_str(), ORIGIN, StatusCode::SEE_OTHER),
        (session.as_str(), ORIGIN, StatusCode::SEE_OTHER),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/me/books/{book}/chapters/illustrated"))
                    .method("POST")
                    .header(header::ORIGIN, origin)
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::COOKIE, cookie(who, &csrf))
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let picture = pg(
        &url,
        &format!(
            "SELECT c.id || '|' || v.id || '|' ||
                    CASE WHEN v.body_markdown IS NULL THEN 'NULL' ELSE 'BODY' END ||
                    '|' || a.storage_object_id::text
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id
             JOIN chapter_version_assets a
               ON a.chapter_id=c.id AND a.version_id=v.id
             WHERE c.book_id={book} AND c.chapter_no=3"
        ),
    );
    let fields = picture.split('|').collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[2], "NULL", "illustrated V1 must never store PG Markdown");
    let image_chapter: i64 = fields[0].parse().unwrap();
    let image_version: i64 = fields[1].parse().unwrap();
    let object_id = tor_contracts::paid_delivery::ObjectId::parse(fields[3]).unwrap();
    let asset = PgPaidAssets::new(pool.clone())
        .encrypted_for_pinned(
            ChapterVersionRef {
                chapter_id: PositiveId::new(image_chapter).unwrap(),
                version_id: PositiveId::new(image_version).unwrap(),
            },
            &object_id,
        )
        .await
        .unwrap()
        .expect("B must bind only the sealed image to its published V1");
    let sealed_png = store.get_verified(&asset.object).await.unwrap();
    assert!(!sealed_png
        .windows(png.len())
        .any(|part| part == png.as_slice()));
    let (image_status, image_editor) = browse(
        &app,
        &format!("/me/books/{book}/chapters/{image_chapter}/edit"),
        &session,
        &csrf,
    )
    .await;
    assert_eq!(image_status, StatusCode::OK);
    assert!(image_editor.contains(&format!("asset:{}", object_id.as_str())));

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
                    .uri(edit_path.as_str())
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
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
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
    assert!(PgPaidStorage::new(pool)
        .encrypted_chapter(first)
        .await
        .unwrap()
        .is_some());
    let (status, page) = browse(&app, &edit_path, &session, &csrf).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Paid bodyless V2 source"));
    assert!(!page.contains(initial_body));
}
