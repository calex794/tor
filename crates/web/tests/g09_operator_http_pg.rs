//! G-09: real HTTP authorization with disposable PG; no S3 operations.
#![cfg(feature = "s3-paid")]

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    env,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_storage::repositories::s3::S3PrivateStore;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let output = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .env("PGCONNECT_TIMEOUT", "5")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=8000 -c lock_timeout=5000",
        )
        .output()
        .expect("disposable PG fixture needs psql");
    assert!(
        output.status.success(),
        "SQL fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    session: Option<&str>,
    csrf: &str,
    posted: &str,
    origin: &str,
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::ORIGIN, origin);
    if let Some(token) = session {
        builder = builder.header(
            header::COOKIE,
            format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
        );
    }
    let body = if method == "POST" {
        builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        Body::from(format!("csrf_token={posted}"))
    } else {
        Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci and preinstalled G-09 migrations"]
async fn g09_http_guest_member_csrf_origin_and_revoked_admin_are_denied() {
    let url = env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never touch durable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    // This fixture sends no requests to S3. A dummy client is sufficient to
    // prove the auth boundary without creating an object or needing credentials.
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .endpoint_url("http://127.0.0.1:9000")
        .force_path_style(true)
        .build();
    let store = S3PrivateStore::new(
        aws_sdk_s3::Client::from_conf(config),
        "tor-g09-private-ci".to_owned(),
    )
    .unwrap();
    let app =
        tor_web::g09_operator::router(PgAccounts::from_pool(pool), store, false, ORIGIN.to_owned());

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('g09-operator-{nonce}@example.invalid','G09 Operator CI','$argon2id$ci')
         RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let session = generate_session_token();
    let csrf = generate_session_token();
    let digest = hex::encode(session_token_digest(&session));
    pg(
        &url,
        &format!(
            "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES(decode('{digest}','hex'),{actor},clock_timestamp()+interval '1 day')"
        ),
    );
    let path = "/admin/storage/g09";
    let command = "/admin/storage/g09/pg-to-s3/chapter/next";
    let body_command = "/admin/storage/g09/get-sha/chapter/run";
    assert_eq!(
        request(&app, "GET", path, None, &csrf, &csrf, ORIGIN).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "POST", command, None, &csrf, &csrf, ORIGIN).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, "GET", path, Some(&session), &csrf, &csrf, ORIGIN).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", command, Some(&session), &csrf, &csrf, ORIGIN).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            body_command,
            Some(&session),
            &csrf,
            &csrf,
            ORIGIN
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", body_command, None, &csrf, &csrf, ORIGIN).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "POST",
            body_command,
            Some(&session),
            &csrf,
            &csrf,
            ORIGIN
        )
        .await,
        StatusCode::FORBIDDEN
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({actor},'administrator',true)"
        ),
    );
    assert_eq!(
        request(&app, "GET", path, Some(&session), &csrf, &csrf, ORIGIN).await,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            body_command,
            Some(&session),
            &csrf,
            "wrong",
            ORIGIN
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            body_command,
            Some(&session),
            &csrf,
            &csrf,
            "https://attacker.invalid"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/admin/storage/g09/get-sha/cover/run",
            Some(&session),
            &csrf,
            &csrf,
            ORIGIN
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "POST",
            command,
            Some(&session),
            &csrf,
            "wrong",
            ORIGIN
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            command,
            Some(&session),
            &csrf,
            &csrf,
            "https://attacker.invalid"
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            "/admin/storage/g09/delete/chapter/next",
            Some(&session),
            &csrf,
            &csrf,
            ORIGIN
        )
        .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, "POST", command, Some(&session), &csrf, &csrf, ORIGIN).await,
        StatusCode::CONFLICT
    );
    pg(
        &url,
        &format!(
        "UPDATE account_roles SET is_active=FALSE WHERE user_id={actor} AND role='administrator'"
    ),
    );
    assert_eq!(
        request(&app, "GET", path, Some(&session), &csrf, &csrf, ORIGIN).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, "POST", command, Some(&session), &csrf, &csrf, ORIGIN).await,
        StatusCode::FORBIDDEN
    );
}
