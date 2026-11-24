//! FND-00 / #349: boot the FREE candidate profile and let its actual S023 worker publish.
use super::Fixture;
use reqwest::{header, Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env,
    io::Read,
    net::TcpListener,
    num::NonZeroU32,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};

const STAGING: &str = "tor-fnd00-candidate-staging";
const CANONICAL: &str = "tor-fnd00-candidate-canonical";
const S3_ENDPOINT: &str = "http://127.0.0.1:9000";
const BODY: &str = "Текст внутренней проверки 😀.";

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(fixture: &Fixture) -> (Command, String) {
    let local = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let origin = format!("http://{local}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_tor-web"));
    // Never inherit a provider, credentials, or operator opt-ins from the runner.
    command
        .env_clear()
        .env("DATABASE_URL", &fixture.url)
        .env("TOR_AUTH_ENABLED", "1")
        .env("TOR_PUBLIC_ORIGIN", &origin)
        .env("TOR_LISTEN_ADDR", local.to_string())
        .env("TOR_CHAPTER_IMPORT_WORKER_ENABLED", "1")
        .env("TOR_CHAPTER_IMPORT_S3_BUCKET", STAGING)
        .env("TOR_FREE_S3_PUBLIC_ORIGIN", S3_ENDPOINT)
        .env("AWS_ENDPOINT_URL_S3", S3_ENDPOINT)
        .env("AWS_REGION", "us-east-1")
        .env("AWS_EC2_METADATA_DISABLED", "true")
        .env(
            "AWS_ACCESS_KEY_ID",
            env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
        )
        .env(
            "AWS_SECRET_ACCESS_KEY",
            env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
        )
        .stdout(Stdio::null());
    (command, origin)
}

async fn post(client: &Client, origin: &str, route: &str, cookie: &str, input: Value) -> Value {
    let response = client
        .post(format!("{origin}{route}"))
        .header(header::ORIGIN, origin)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(input.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{route}: {body}");
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and loopback MinIO"]
async fn fnd00_free_candidate_runtime_uses_bodyless_s023() {
    assert!(
        env::var("TEST_DATABASE_URL").as_deref()
            == Ok("postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_test"),
        "requires the disposable CI database"
    );
    let fixture = Fixture::new_in_bucket("FND-00 candidate runtime", STAGING).await;
    let pool = fixture.catalog.pool();
    let (mut without_bucket, _) = command(&fixture);
    let mut denied = Server(without_bucket.stderr(Stdio::piped()).spawn().unwrap());
    let exit = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(status) = denied.0.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("queue must not boot without canonical FREE storage");
    let mut reason = String::new();
    denied
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut reason)
        .unwrap();
    assert!(!exit.success(), "{reason}");
    assert!(
        reason.contains("S3-free import worker requires configured TOR_FREE_S3_BUCKET"),
        "{reason}"
    );
    assert_eq!(fixture.count("chapter_versions"), 0);

    fixture
        .client
        .create_bucket()
        .bucket(CANONICAL)
        .send()
        .await
        .unwrap();
    let token = generate_session_token();
    let csrf = generate_session_token();
    PgAccounts::from_pool(pool.clone())
        .insert_session(
            fixture.actor,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let (mut with_bucket, origin) = command(&fixture);
    let mut server = Server(
        with_bucket
            .env("TOR_FREE_S3_BUCKET", CANONICAL)
            .spawn()
            .unwrap(),
    );
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(server.0.try_wait().unwrap().is_none(), "server exited");
            if client
                .get(format!("{origin}/readyz"))
                .send()
                .await
                .is_ok_and(|response| response.status() == StatusCode::OK)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("candidate TOR must be ready");
    let route = format!("/me/books/{}/chapters/import", fixture.book);
    let input = json!({
        "filename": "chapter.txt",
        "text": format!("Глава 1: Начало\n{BODY}"),
        "csrf_token": csrf,
    });
    let preview = post(
        &client,
        &origin,
        &format!("{route}-preview"),
        &cookie,
        input.clone(),
    )
    .await;
    assert_eq!(preview["count"], 1);
    let mut command = input;
    command["fingerprint"] = preview["fingerprint"].clone();
    command["command_key"] = json!(generate_session_token());
    let accepted = post(
        &client,
        &origin,
        &format!("{route}-queue"),
        &cookie,
        command,
    )
    .await;
    let job_id = accepted["job_id"].as_i64().unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            assert!(server.0.try_wait().unwrap().is_none(), "server exited");
            let (state, saved): (String, Option<i32>) =
                sqlx::query_as("SELECT status,saved_count FROM chapter_import_jobs WHERE id=$1")
                    .bind(job_id)
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if state == "succeeded" {
                assert_eq!(saved, Some(1));
                break;
            }
            assert!(!matches!(state.as_str(), "failed" | "cancelled"), "{state}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the running TOR worker must finish the HTTP-enqueued job");

    type StoredEdition = (String, String, Vec<u8>, Vec<u8>, bool);
    let (bucket, key, stored_sha, version_sha, is_bodyless): StoredEdition = sqlx::query_as(
        "SELECT f.bucket,f.object_key,f.sha256,v.content_sha256,v.body_markdown IS NULL
         FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
         JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
         JOIN free_chapter_storage_objects f ON f.chapter_id=c.id AND f.version_id=v.id
         WHERE c.book_id=$1",
    )
    .bind(fixture.book)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(fixture.count("chapter_versions"), 1);
    assert_eq!(bucket, CANONICAL, "staging is not canonical storage");
    assert!(is_bodyless, "no legacy plaintext may be written");
    let sha: [u8; 32] = Sha256::digest(BODY.as_bytes()).into();
    assert_eq!(stored_sha.as_slice(), &sha[..]);
    assert_eq!(version_sha.as_slice(), &sha[..]);
    let actual = fixture
        .client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes();
    assert_eq!(actual.as_ref(), BODY.as_bytes());
    println!("FND00_FREE_CANDIDATE_RUNTIME_OK");
}
