//! G-07/S066: real HTTP and disposable PG acceptance of administrator retries.
use axum::{
    body::{to_bytes, Body},
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
use tor_reading::repositories::PgReader;
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
        .expect("disposable PostgreSQL fixture requires psql");
    assert!(
        output.status.success(),
        "SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn dead_event(url: &str, key: &str) -> String {
    pg(
        url,
        &format!(
            "INSERT INTO outbox_events (
                event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,required_consumers,attempts,lease_epoch,dead_at)
             VALUES ('ChapterPublished',1,'chapter',0,'{key}','{{}}'::jsonb,
                     ARRAY['chapter_release_fanout_v1'],12,7,clock_timestamp())
             RETURNING id"
        ),
    )
}

async fn request(
    app: &Router,
    id: &str,
    session: Option<&str>,
    csrf_cookie: &str,
    csrf_header: Option<&str>,
    origin: &str,
) -> (StatusCode, String) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/jobs/{id}/retry"))
        .header(header::ORIGIN, origin);
    if let Some(token) = session {
        builder = builder.header(
            header::COOKIE,
            format!("tor-dev-session={token}; tor-dev-csrf={csrf_cookie}"),
        );
    }
    if let Some(value) = csrf_header {
        builder = builder.header("x-csrf-token", value);
    }
    eprintln!("G07_RETRY_REQUEST_START id={id}");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(12),
        app.clone().oneshot(builder.body(Body::empty()).unwrap()),
    )
    .await
    .expect("G-07 retry HTTP handler did not complete within 12 seconds")
    .unwrap();
    eprintln!("G07_RETRY_REQUEST_END status={}", response.status());
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_07_admin_http_retry_is_authorized_fenced_audited_and_idempotent() {
    let url = env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never mutate a durable database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        PgAccounts::from_pool(pool.clone()),
        false,
        ORIGIN.to_owned(),
    );

    eprintln!("G07_RETRY_STAGE fixture creation");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor_id: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('g-retry-{nonce}@example.invalid','G Retry Admin','$argon2id$ci')
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
             VALUES(decode('{digest}','hex'),{actor_id},clock_timestamp()+interval '1 day')"
        ),
    );
    let event_id = dead_event(&url, &format!("g-admin-retry-{nonce}"));
    pg(
        &url,
        &format!(
            "INSERT INTO event_deliveries(consumer,event_id)
             VALUES('legacy-fanout-v0','{event_id}'::uuid)"
        ),
    );

    eprintln!("G07_RETRY_STAGE identity and outbox fixture created");
    // No session and ordinary members never receive operator rights.
    assert_eq!(
        request(&app, &event_id, None, &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, &event_id, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({actor_id},'administrator',true)"
        ),
    );
    // Credential alone is insufficient: token and same-origin are mandatory.
    assert_eq!(
        request(&app, &event_id, Some(&session), &csrf, None, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            &event_id,
            Some(&session),
            &csrf,
            Some(&csrf),
            "https://evil.invalid"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "not-a-uuid",
            Some(&session),
            &csrf,
            Some(&csrf),
            ORIGIN
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    eprintln!("G07_RETRY_STAGE authorized retry");
    let (status, body) = request(&app, &event_id, Some(&session), &csrf, Some(&csrf), ORIGIN).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.contains("\"status\":\"queued\""));
    assert!(body.contains("\"previous_attempts\":12"));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT (dead_at IS NULL)::text || '/' || attempts::text || '/'
                        || lease_epoch::text
                 FROM outbox_events WHERE id='{event_id}'::uuid"
            ),
        ),
        "true/0/8"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM event_deliveries WHERE event_id='{event_id}'::uuid"),
        ),
        "1",
        "manual requeue must retain existing consumer acknowledgements"
    );
    assert_eq!(
        request(&app, &event_id, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::CONFLICT,
        "a second request must not schedule duplicate work"
    );

    eprintln!("G07_RETRY_STAGE first retry and receipts checked");
    // Two simultaneous operator requests race on the same event row.
    let concurrent = dead_event(&url, &format!("g-admin-concurrent-{nonce}"));
    let (first, second) = tokio::join!(
        request(
            &app,
            &concurrent,
            Some(&session),
            &csrf,
            Some(&csrf),
            ORIGIN
        ),
        request(
            &app,
            &concurrent,
            Some(&session),
            &csrf,
            Some(&csrf),
            ORIGIN
        )
    );
    let statuses = [first.0, second.0];
    assert!(statuses.contains(&StatusCode::ACCEPTED));
    assert!(statuses.contains(&StatusCode::CONFLICT));

    eprintln!("G07_RETRY_STAGE concurrency checked");
    // Unsupported jobs remain dead-lettered, without successful audit rows.
    let unknown = dead_event(&url, &format!("g-admin-unknown-{nonce}"));
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET event_type='payment.capture' WHERE id='{unknown}'::uuid"
        ),
    );
    assert_eq!(
        request(&app, &unknown, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let external = dead_event(&url, &format!("g-admin-external-{nonce}"));
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET required_consumers=ARRAY['external_mail']::text[]
             WHERE id='{external}'::uuid"
        ),
    );
    assert_eq!(
        request(&app, &external, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM g_outbox_admin_retry_audit
                 WHERE event_id IN ('{unknown}'::uuid, '{external}'::uuid)"
            ),
        ),
        "0",
        "rejected retries must not generate success audit rows"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE id IN ('{unknown}'::uuid, '{external}'::uuid)
                   AND dead_at IS NOT NULL AND attempts=12 AND lease_epoch=7"
            ),
        ),
        "2",
        "rejected retries cannot change original event state"
    );
    let child = dead_event(&url, &format!("g-admin-child-{nonce}"));
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET event_type='reader.chapter.released',
             required_consumers=ARRAY['user_notifications']::text[]
             WHERE id='{child}'::uuid"
        ),
    );
    assert_eq!(
        request(&app, &child, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::ACCEPTED,
        "the reviewed in-app notification handler must remain retryable"
    );

    let active = dead_event(&url, &format!("g-admin-active-{nonce}"));
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET lease_until=clock_timestamp()+interval '5 minutes'
             WHERE id='{active}'::uuid"
        ),
    );
    assert_eq!(
        request(&app, &active, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::CONFLICT,
        "a live lease cannot be stolen by an administrator"
    );

    eprintln!("G07_RETRY_STAGE active lease checked");
    let revoked = dead_event(&url, &format!("g-admin-revoked-{nonce}"));
    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={actor_id} AND role='administrator'"
        ),
    );
    assert_eq!(
        request(&app, &revoked, Some(&session), &csrf, Some(&csrf), ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "role revocation must be checked on every request"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT (dead_at IS NOT NULL)::text FROM outbox_events WHERE id='{revoked}'::uuid"
            ),
        ),
        "true"
    );

    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM g_outbox_admin_retry_audit WHERE event_id='{event_id}'::uuid"
            ),
        ),
        "1",
        "one successful retry must have exactly one append-only audit row"
    );
    eprintln!("G07_RETRY_STAGE done");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT admin_user_id FROM g_outbox_admin_retry_audit
                 WHERE event_id='{event_id}'::uuid"
            ),
        ),
        actor_id.to_string(),
        "every retry must identify its administrator"
    );
    // Audit immutability is enforced in PostgreSQL, not just an API convention.
    let mutation = sqlx::query(
        "UPDATE g_outbox_admin_retry_audit SET previous_attempts=0
         WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await;
    assert!(
        mutation.unwrap_err().to_string().contains("append-only"),
        "UPDATE of the retry audit must be blocked by the database trigger"
    );
    let deletion = sqlx::query("DELETE FROM g_outbox_admin_retry_audit WHERE event_id=$1::uuid")
        .bind(&event_id)
        .execute(&pool)
        .await;
    assert!(
        deletion.unwrap_err().to_string().contains("append-only"),
        "DELETE of the retry audit must be blocked by the database trigger"
    );
    pool.close().await;
}
