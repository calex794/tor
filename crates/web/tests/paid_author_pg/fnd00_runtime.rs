//! FND-00h: live paid S3-only process configuration and restart acceptance.
use super::*;

// FND-00h: the production entry point, rather than a manually wired test
// router, must select the bodyless writer only with explicit operator opt-in.
struct PaidRuntime(std::process::Child);

impl Drop for PaidRuntime {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn runtime_command(url: &str, bucket: &str) -> (Command, String) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let local = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let origin = format!("http://{local}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tor-web"));
    // No inherited operator flags, external credentials or provider endpoints.
    // The caller has already attested the disposable DB and loopback MinIO.
    command
        .env_clear()
        .env("DATABASE_URL", url)
        .env("TOR_AUTH_ENABLED", "1")
        .env("TOR_LISTEN_ADDR", local.to_string())
        .env("TOR_PUBLIC_ORIGIN", &origin)
        .env("TOR_PAID_GRANTS_ENABLED", "1")
        .env("TOR_PAID_AUTHOR_PUBLISH_ENABLED", "1")
        .env("TOR_PAID_S3_BUCKET", bucket)
        .env("TOR_PAID_S3_PUBLIC_ORIGIN", ENDPOINT)
        .env("TOR_CONTENT_KEK_V1", URL_SAFE_NO_PAD.encode([0x67; 32]))
        .env("AWS_ENDPOINT_URL_S3", ENDPOINT)
        .env("AWS_REGION", "us-east-1")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env("AWS_ACCESS_KEY_ID", env::var("TOR_TEST_S3_ACCESS_KEY").unwrap())
        .env(
            "AWS_SECRET_ACCESS_KEY",
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
        )
        .stdout(std::process::Stdio::null());
    (command, origin)
}

async fn start_paid_runtime(
    mut command: Command,
    origin: &str,
    http: &reqwest::Client,
) -> PaidRuntime {
    let mut server = PaidRuntime(command.spawn().unwrap());
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(server.0.try_wait().unwrap().is_none(), "TOR exited");
            if http
                .get(format!("{origin}/readyz"))
                .send()
                .await
                .is_ok_and(|response| response.status() == reqwest::StatusCode::OK)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("isolated paid TOR must become ready");
    server
}

async fn assert_rejected_boot(mut command: Command, expected: &str) {
    use std::io::Read;
    let mut server = PaidRuntime(command.stderr(std::process::Stdio::piped()).spawn().unwrap());
    let status = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(status) = server.0.try_wait().unwrap() {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("invalid paid configuration must not start");
    let mut reason = String::new();
    server
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut reason)
        .unwrap();
    assert!(!status.success(), "invalid configuration unexpectedly succeeded");
    assert!(
        reason.contains(expected),
        "startup failed for an unexpected reason"
    );
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn fnd00_paid_runtime_explicit_opt_in_and_restart_preserve_bodyless_source() {
    let url = env::var("TEST_DATABASE_URL").expect("disposable CI database");
    assert_eq!(url, "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci");
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    let bucket = "tor-fnd00-paid-runtime-ci";
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "local-paid-runtime",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let s3 = Client::from_conf(config);
    s3.create_bucket().bucket(bucket).send().await.unwrap();

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, session) = user(&accounts, "fnd00-runtime-owner@example.invalid").await;
    let (_, outsider) = user(&accounts, "fnd00-runtime-outsider@example.invalid").await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:fnd00-paid-runtime','ru','Runtime paid editor','public',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    // Leave the application process its own six-connection DB budget.
    pool.close().await;

    let (mut invalid, _) = runtime_command(&url, bucket);
    invalid
        .env("TOR_PAID_AUTHOR_S3_ONLY_ENABLED", "1")
        .env_remove("TOR_PAID_AUTHOR_PUBLISH_ENABLED");
    assert_rejected_boot(
        invalid,
        "Paid author S3-only mode requires enabled paid author publication",
    )
    .await;
    let (mut invalid, _) = runtime_command(&url, bucket);
    invalid
        .env("TOR_PAID_AUTHOR_S3_ONLY_ENABLED", "1")
        .env_remove("TOR_CONTENT_KEK_V1");
    assert_rejected_boot(invalid, "Invalid private content KEK V1/V2 configuration").await;

    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let csrf = generate_session_token();
    let key = generate_session_token();
    let body = "FND00 runtime private bodyless V1";
    let create = format!(
        "number=1&title=Runtime%20paid&body=FND00%20runtime%20private%20bodyless%20V1&access=paid&price_amount=100&command_key={key}&csrf_token={csrf}"
    );
    let (mut enabled, origin) = runtime_command(&url, bucket);
    enabled.env("TOR_PAID_AUTHOR_S3_ONLY_ENABLED", "1");
    let runtime = start_paid_runtime(enabled, &origin, &http).await;
    let route = format!("/me/books/{book}/chapters");
    for _ in 0..2 {
        let response = http
            .post(format!("{origin}{route}"))
            .header(header::ORIGIN, origin.as_str())
            .header(header::COOKIE, cookie(&session, &csrf))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(create.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    let (chapter, version): (i64, i64) = serde_json::from_str(&pg(
        &url,
        &format!(
            "SELECT json_build_array(c.id,p.version_id)
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.book_id={book}"
        ),
    ))
    .unwrap();
    let snapshot = || {
        pg(
            &url,
            &format!(
                "SELECT p.version_id||'|'||c.edit_revision||'|'||count(v.id)||'|'||
                        count(v.body_markdown)||'|'||count(DISTINCT v.storage_object_id)
                 FROM chapter_publications p JOIN chapters c ON c.id=p.chapter_id
                 JOIN chapter_versions v ON v.chapter_id=c.id
                 WHERE c.id={chapter} GROUP BY p.version_id,c.edit_revision"
            ),
        )
    };
    let exact = format!("{version}|1|1|0|1");
    assert_eq!(snapshot(), exact);
    let edit = format!("{route}/{chapter}/edit");
    let page = http
        .get(format!("{origin}{edit}"))
        .header(header::COOKIE, cookie(&session, &csrf))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.text().await.unwrap().contains(body));
    let denied = http
        .get(format!("{origin}{edit}"))
        .header(header::COOKIE, cookie(&outsider, &csrf))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    assert!(!denied.text().await.unwrap().contains(body));
    drop(runtime);

    // Omit the S3-only flag (default OFF) while grants and author remain
    // enabled. A restarted legacy writer must not mutate the bodyless V1.
    let (disabled, origin) = runtime_command(&url, bucket);
    let runtime = start_paid_runtime(disabled, &origin, &http).await;
    let rollback_key = generate_session_token();
    let rejected = http
        .post(format!("{origin}{edit}"))
        .header(header::ORIGIN, origin.as_str())
        .header(header::COOKIE, cookie(&session, &csrf))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!(
            "number=1&title=Runtime%20paid&body=Must%20not%20write%20to%20PG&expected_revision=1&command_key={rollback_key}&csrf_token={csrf}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(snapshot(), exact);
    drop(runtime);

    let (mut enabled, origin) = runtime_command(&url, bucket);
    enabled.env("TOR_PAID_AUTHOR_S3_ONLY_ENABLED", "1");
    let runtime = start_paid_runtime(enabled, &origin, &http).await;
    let restored = http
        .get(format!("{origin}{edit}"))
        .header(header::COOKIE, cookie(&session, &csrf))
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);
    assert!(restored.text().await.unwrap().contains(body));
    assert_eq!(snapshot(), exact);
    drop(runtime);
    std::io::Write::write_all(
        &mut std::io::stdout().lock(),
        b"FND00_PAID_RUNTIME_OPT_IN_RESTART_OK\n",
    )
    .unwrap();
}
