//! OBS-09: REAL private S3 (MinIO) + REAL PostgreSQL + asynchronous worker
//! + public HTTP, all on a disposable local CI instance. Never run on a VPS.
#![cfg(feature = "s3-covers")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::{io::Cursor, sync::Arc, time::Duration};
use tor_books::repositories::{
    cover_publications::PgPublicCoverPublications,
    cover_review::PgCoverReview,
    cover_slots::{PgCoverSlots, PublicCoverKind},
    PgCatalog,
};
use tor_ingest::{
    repositories::{cover_jobs::PgCoverJobs, s3::S3CoverStore},
    services::{
        cover_jobs::{CoverJobs, CoverObjectStore},
        image_pipeline::RECIPE_VERSION,
    },
};
use tor_reading::repositories::PgReader;
#[cfg(feature = "test-cdn-loopback")]
use tor_web::yandex_cdn_purge::YandexCdnPurge;
use tor_web::{
    cover_cdn_cleanup::CoverOriginCleanup,
    cover_cdn_purge::{CoverCdnApi, CoverCdnPurgeWorker},
    cover_delivery::CoverDelivery,
    cover_worker::{CoverRunner, CoverRunnerMonitor},
};
use tower::ServiceExt;

#[cfg(not(feature = "test-cdn-loopback"))]
#[path = "cover_s3_pg/cdn_fake.rs"]
mod cdn_fake;
#[path = "cover_s3_pg/cdn_html.rs"]
mod cdn_html;
#[cfg(feature = "test-cdn-loopback")]
#[path = "cover_s3_pg/cdn_wire.rs"]
mod cdn_wire;
#[path = "cover_s3_pg/private_cleanup.rs"]
mod private_cleanup;
#[path = "cover_s3_pg/source_cleanup.rs"]
mod source_cleanup;

const BUCKET: &str = "tor-private-obs09-ci";
const BOOK_ID: i64 = 950;
const SOURCE_KEY: &str = "staging/covers/sources/obs09-owner-950.png";

fn source_png() -> Vec<u8> {
    let image = RgbImage::from_fn(120, 190, |x, y| Rgb([41, x as u8, y as u8]));
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut output, ImageFormat::Png)
        .expect("small CI PNG");
    output.into_inner()
}

fn digest(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

async fn request(app: &axum::Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .expect("local router must return HTTP response")
}

async fn moderation_request(
    app: &axum::Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

// Run lifecycle mutations outside the async PostgreSQL executor. A synchronous
// psql call on its worker thread can deadlock a single-threaded Tokio test.
async fn pg_sql(url: &str, sql: &str) -> String {
    let url = url.to_owned();
    let sql = sql.to_owned();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new("psql")
            .args([
                "-X",
                "-q",
                "-t",
                "-A",
                "-v",
                "ON_ERROR_STOP=1",
                "-d",
                &url,
                "-c",
                &sql,
            ])
            .output()
            .expect("disposable CI psql");
        assert!(
            output.status.success(),
            "disposable CI SQL failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF-8 psql output")
            .trim()
            .to_owned()
    })
    .await
    .expect("blocking SQL task")
}

fn local_client() -> Client {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("loopback S3 endpoint");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "OBS-09 must never contact an external provider"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("CI-only username");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("CI-only password");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(access, secret, None, None, "obs09-ci"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(2))
        .build();
    Client::from_conf(config)
}

#[tokio::test]
#[ignore = "requires isolated PG18, seeded owner/book 950 and private loopback MinIO"]
async fn obs09_private_s3_postgres_worker_http_and_tamper_denial() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(
        url.contains("/tor_ci"),
        "OBS-09 must run only against the disposable tor_ci database"
    );
    let catalog = PgCatalog::connect(&url)
        .await
        .expect("CI database connection");
    let slots = PgCoverSlots::new(catalog.pool());
    let jobs = PgCoverJobs::new(catalog.pool());
    let client = local_client();
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("create unlisted disposable private bucket");
    let store = S3CoverStore::new(client.clone(), BUCKET.to_owned()).unwrap();

    let png = source_png();
    let hash = digest(&png);
    store
        .put_source_verified(SOURCE_KEY, &png, "image/png", &hash)
        .await
        .expect("real signed PUT + HEAD + bounded digest-checked GET");
    let source = jobs
        .register_verified_source(
            BOOK_ID,
            BOOK_ID,
            SOURCE_KEY,
            "image/png",
            png.len() as i64,
            &hash,
        )
        .await
        .expect("trusted PostgreSQL source receipt");
    let job = jobs
        .enqueue(source, BOOK_ID, BOOK_ID, 0, RECIPE_VERSION)
        .await
        .expect("durable fenced cover job");

    let app = tor_web::router_with_postgres_reading_covers(
        catalog.clone(),
        PgReader::new(catalog.pool()),
        Arc::new(store.clone()),
    );
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::NOT_FOUND,
        "an S3 source is not automatically a publicly attached cover"
    );

    // Only the real background runner may claim/verify/store/attach both
    // WebP variants. The test never calls CoverWorker::run_one manually.
    let monitor = CoverRunnerMonitor::default();
    let handle = CoverRunner::new(catalog.clone(), Arc::new(store.clone()), monitor.clone())
        .spawn(Duration::from_millis(40), Duration::from_secs(90));
    let completion = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            if monitor.snapshot().attached >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    })
    .await;
    handle.abort();
    let _ = handle.await;
    completion.expect("background runner must attach a cover from real S3 + PG");
    let snapshot = monitor.snapshot();
    assert_eq!(snapshot.attached, 1);
    assert_eq!(snapshot.timeouts, 0);
    assert_eq!(snapshot.errors, 0);
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), None);
    assert_eq!(
        jobs.job_status(job).await.unwrap().as_deref(),
        Some("done"),
        "processed artifacts must wait for site moderation"
    );
    let accounts = tor_accounts::repositories::PgAccounts::from_pool(catalog.pool());
    let token = tor_accounts::services::security::generate_session_token();
    let session_digest = tor_accounts::services::security::session_token_digest(&token);
    let reviewer = accounts
        .register_with_session(
            &format!(
                "obs09-cover-moderator-{}@example.invalid",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            "Cover reviewer",
            &tor_accounts::services::security::hash_password("Obs09CoverMod2026!").unwrap(),
            &session_digest,
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg_sql(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({reviewer},'moderator',true)"
        ),
    )
    .await;
    // Two independent live site moderators race for the same immutable request.
    let second_token = tor_accounts::services::security::generate_session_token();
    let second_session_digest =
        tor_accounts::services::security::session_token_digest(&second_token);
    let second_reviewer = accounts
        .register_with_session(
            &format!(
                "obs09-second-cover-moderator-{}@example.invalid",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            "Independent second cover reviewer",
            &tor_accounts::services::security::hash_password("Obs09SecondMod2026!").unwrap(),
            &second_session_digest,
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(reviewer, second_reviewer);
    pg_sql(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({second_reviewer},'moderator',true)"
        ),
    )
    .await;
    let reviews = PgCoverReview::new(catalog.pool());
    let review_request = reviews
        .pending(&session_digest, None)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.job_id == job)
        .expect("private cover candidate");
    assert_eq!(review_request.expected_slot_revision, 0);
    assert_eq!(review_request.book_id, BOOK_ID);
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::NOT_FOUND,
        "completed image processing cannot publish without moderation"
    );
    // The live site-role/CSRF HTTP path must approve; no synthetic B call.
    let origin = "http://127.0.0.1:3098";
    let moderator_app = tor_web::router_with_postgres_reading_auth_cover_uploads(
        catalog.clone(),
        PgReader::new(catalog.pool()),
        accounts.clone(),
        false,
        origin.to_owned(),
        Arc::new(store.clone()),
    );
    let csrf = tor_accounts::services::security::generate_session_token();
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let second_csrf = tor_accounts::services::security::generate_session_token();
    let second_cookie = format!("tor-dev-session={second_token}; tor-dev-csrf={second_csrf}");
    assert_eq!(
        moderation_request(&moderator_app, "GET", "/moderation/covers", "", origin, "")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let queue = moderation_request(
        &moderator_app,
        "GET",
        "/moderation/covers",
        &cookie,
        origin,
        "",
    )
    .await;
    assert_eq!(queue.status(), StatusCode::OK);
    assert_eq!(queue.headers()[header::CACHE_CONTROL], "no-store");
    let html = to_bytes(queue.into_body(), 262144).await.unwrap();
    assert!(String::from_utf8_lossy(&html).contains("Первая обложка"));
    let review_form_path = format!("/moderation/covers/{}/decision", review_request.id);
    let recent = moderation_request(
        &moderator_app,
        "GET",
        "/moderation/covers?q=950&wait=lt24",
        &cookie,
        origin,
        "",
    )
    .await;
    assert_eq!(recent.status(), StatusCode::OK);
    let recent_html = to_bytes(recent.into_body(), 262144).await.unwrap();
    assert!(String::from_utf8_lossy(&recent_html).contains(&review_form_path));
    let too_old = moderation_request(
        &moderator_app,
        "GET",
        "/moderation/covers?q=950&wait=gte72",
        &cookie,
        origin,
        "",
    )
    .await;
    assert_eq!(too_old.status(), StatusCode::OK);
    let old_html = to_bytes(too_old.into_body(), 262144).await.unwrap();
    assert!(!String::from_utf8_lossy(&old_html).contains(&review_form_path));
    assert_eq!(
        moderation_request(
            &moderator_app,
            "GET",
            "/moderation/covers?wait=unknown",
            &cookie,
            origin,
            "",
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    pg_sql(
        &url,
        &format!(
            "UPDATE book_cover_review_requests
             SET submitted_at=now()-interval '5 days' WHERE id={}",
            review_request.id
        ),
    )
    .await;
    let aged = moderation_request(
        &moderator_app,
        "GET",
        "/moderation/covers?q=950&kind=first&wait=gte72",
        &cookie,
        origin,
        "",
    )
    .await;
    assert_eq!(aged.status(), StatusCode::OK);
    let aged_html = to_bytes(aged.into_body(), 262144).await.unwrap();
    assert!(String::from_utf8_lossy(&aged_html).contains(&review_form_path));
    let preview = moderation_request(
        &moderator_app,
        "GET",
        &format!("/moderation/covers/{}/preview", review_request.id),
        &cookie,
        origin,
        "",
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(preview.headers()[header::CONTENT_TYPE], "image/webp");
    assert_eq!(preview.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(preview.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let preview_bytes = to_bytes(preview.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert!(preview_bytes.starts_with(b"RIFF"));
    assert_eq!(
        moderation_request(
            &moderator_app,
            "POST",
            &format!("/moderation/covers/{}/decision", review_request.id),
            &cookie,
            "https://evil.invalid",
            &format!("csrf_token={csrf}&expected_slot_revision=0&action=approve"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let cover_path = format!("/moderation/covers/{}/decision", review_request.id);
    let cover_body = format!("csrf_token={csrf}&expected_slot_revision=0&action=approve");
    let preview = moderation_request(
        &moderator_app,
        "POST",
        &cover_path,
        &cookie,
        origin,
        &cover_body,
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(preview.headers()[header::CACHE_CONTROL], "no-store");
    let preview_html = to_bytes(preview.into_body(), 262144).await.unwrap();
    assert!(String::from_utf8_lossy(&preview_html).contains("name=\"confirmed\" value=\"yes\""));
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), None);
    // Simultaneous final HTTP POSTs: PostgreSQL row locks and the slot CAS
    // must yield exactly one approval and one stale request (409).
    let first_confirmed_body = format!("{cover_body}&confirmed=yes");
    let second_confirmed_body =
        format!("csrf_token={second_csrf}&expected_slot_revision=0&action=approve&confirmed=yes");
    let (first_result, second_result) = tokio::join!(
        moderation_request(
            &moderator_app,
            "POST",
            &cover_path,
            &cookie,
            origin,
            &first_confirmed_body
        ),
        moderation_request(
            &moderator_app,
            "POST",
            &cover_path,
            &second_cookie,
            origin,
            &second_confirmed_body
        ),
    );
    let first_status = first_result.status();
    let second_status = second_result.status();
    assert!(
        (first_status == StatusCode::SEE_OTHER && second_status == StatusCode::CONFLICT)
            || (first_status == StatusCode::CONFLICT && second_status == StatusCode::SEE_OTHER),
        "two independent moderators must yield one 303 and one 409; got {first_status} / {second_status}"
    );
    let winning_reviewer = if first_status == StatusCode::SEE_OTHER {
        reviewer
    } else {
        second_reviewer
    };
    let loser = if first_status == StatusCode::CONFLICT {
        first_result
    } else {
        second_result
    };
    assert_eq!(loser.headers()[header::CACHE_CONTROL], "no-store");
    let denied_html = to_bytes(loser.into_body(), 262144).await.unwrap();
    assert!(!String::from_utf8_lossy(&denied_html).contains("staging/covers/"));
    assert_eq!(
        pg_sql(
            &url,
            &format!(
                "SELECT count(*) FROM book_cover_review_decisions WHERE request_id={}",
                review_request.id
            ),
        )
        .await,
        "1",
        "racing moderators must append exactly one immutable decision"
    );
    assert_eq!(
        moderation_request(
            &moderator_app,
            "POST",
            &format!("/moderation/covers/{}/decision", review_request.id),
            &cookie,
            origin,
            &format!("csrf_token={csrf}&expected_slot_revision=0&action=approve&confirmed=yes"),
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "repeat review must not create a second approval"
    );
    let history = reviews
        .private_history(&session_digest, BOOK_ID)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].reviewer_user_id, winning_reviewer);
    assert_eq!(history[0].outcome, "approved");
    assert_eq!(slots.current_slot(BOOK_ID).await.unwrap(), Some((job, 1)));
    let decision_history_path = format!("/moderation/books/{BOOK_ID}/history");
    assert_eq!(
        moderation_request(
            &moderator_app,
            "GET",
            &decision_history_path,
            "",
            origin,
            "",
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let history_response = moderation_request(
        &moderator_app,
        "GET",
        &decision_history_path,
        &cookie,
        origin,
        "",
    )
    .await;
    assert_eq!(history_response.status(), StatusCode::OK);
    assert_eq!(
        history_response.headers()[header::CACHE_CONTROL],
        "no-store"
    );
    let history_page = to_bytes(history_response.into_body(), 262144)
        .await
        .unwrap();
    let history_page = String::from_utf8_lossy(&history_page);
    assert!(history_page.contains("Обложка: Одобрено"));
    assert!(history_page.contains(&format!("(#{winning_reviewer})")));
    assert!(!history_page.contains("staging/covers/"));

    let stored_variants = jobs.existing_variants(job).await.unwrap();
    assert_eq!(stored_variants.len(), 2);
    assert_eq!(stored_variants[0].name, "card");
    assert_eq!(stored_variants[1].name, "main");
    for variant in &stored_variants {
        assert!(store
            .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
            .await
            .expect("signed private S3 HEAD"));
        let image = store
            .get_bounded(&variant.storage_key, variant.size_bytes as usize)
            .await
            .expect("signed bounded S3 GET");
        assert!(image.starts_with(b"RIFF"));
        assert_eq!(image.get(8..12), Some(&b"WEBP"[..]));
        assert_eq!(digest(&image), variant.sha256);
    }

    private_cleanup::verify_private_variant_retention(private_cleanup::PrivateRetentionCase {
        url: &url,
        catalog: &catalog,
        slots: &slots,
        jobs: &jobs,
        store: store.clone(),
        client: &client,
        source,
        reviewer,
        job,
    })
    .await;

    let card = slots
        .public_variant(BOOK_ID, PublicCoverKind::Card)
        .await
        .unwrap()
        .expect("published verified cover");
    assert_eq!(card.revision, 1);
    // The publication ledger must reserve each immutable identity BEFORE S3 COPY.
    // Replays never create a second row, and copied state remains idempotent.
    let ledger = PgPublicCoverPublications::new(catalog.pool());
    // Only site-moderator-approved current variants are eligible for
    // background CDN publication. The worker never enumerates raw staging.
    let candidates = ledger.publication_candidates(3).await.unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|(id, _)| *id == BOOK_ID));
    let mut publication_ids = Vec::new();
    for kind in [PublicCoverKind::Card, PublicCoverKind::Main] {
        let reserved = ledger
            .reserve_current(BOOK_ID, kind)
            .await
            .unwrap()
            .expect("current visible cover reserves one immutable row");
        let again = ledger
            .reserve_current(BOOK_ID, kind)
            .await
            .unwrap()
            .expect("repeated reservation returns same row");
        assert_eq!(reserved.id, again.id);
        assert_eq!(reserved.job_id, job);
        assert_eq!(reserved.slot_revision, 1);
        // A reservation is not proof of S3 copy completion, even when
        // Book/slot/variant are all public and attached.
        assert!(
            ledger
                .current_cdn_copies(&[BOOK_ID], kind)
                .await
                .unwrap()
                .is_empty(),
            "reserved CDN copy is not a publishable URL"
        );
        // An aged reservation belonging to a still-public Book must never be
        // reclaimed for deletion: the publisher could be retrying after a crash.
        pg_sql(
            &url,
            &format!(
                "UPDATE public_cover_publications SET created_at=now()-interval '20 minutes' WHERE id={}",
                reserved.id
            ),
        )
        .await;
        assert!(ledger.claim_removals(10).await.unwrap().is_empty());
        assert!(ledger.confirm_copy(reserved.id).await.unwrap());
        assert!(ledger.confirm_copy(reserved.id).await.unwrap());
        publication_ids.push(reserved.id);
    }
    // Reserved identities are retried only after cooldown, not on every tick.
    assert!(ledger.publication_candidates(3).await.unwrap().is_empty());
    // The CDN bucket is a separate destination: never publish source PNGs or
    // use the private cover bucket as a publicly-readable bucket.
    const PUBLIC_BUCKET: &str = "tor-public-cover-cdn-ci";
    client
        .create_bucket()
        .bucket(PUBLIC_BUCKET)
        .send()
        .await
        .expect("create disposable public-target bucket");
    let publisher = S3CoverStore::new(client.clone(), BUCKET.to_owned())
        .unwrap()
        .with_public_cdn(PUBLIC_BUCKET.to_owned(), "https://cdn.example".to_owned())
        .unwrap();
    // An S3 adapter with a configured CDN is not enough for permission to
    // publish: the durable B-owned publication ledger is mandatory.
    let unregistered = CoverDelivery::new(slots.clone(), Arc::new(publisher.clone()));
    assert!(matches!(
        unregistered
            .publish_cdn_variant(BOOK_ID as u64, PublicCoverKind::Card)
            .await,
        Err(tor_ingest::domain::cover::CoverError::Forbidden)
    ));
    let direct = CoverDelivery::new(slots.clone(), Arc::new(publisher.clone()))
        .with_publication_registry(ledger.clone());
    // The adapter cannot expose a CDN URL without an explicitly installed
    // B-owned publication registry, even when an origin is configured.
    assert!(matches!(
        unregistered
            .current_cdn_urls(&[BOOK_ID as u64], PublicCoverKind::Card)
            .await,
        Err(tor_ingest::domain::cover::CoverError::Forbidden)
    ));
    let mut public_urls = Vec::new();
    for variant in [PublicCoverKind::Card, PublicCoverKind::Main] {
        let url = direct
            .publish_cdn_variant(BOOK_ID as u64, variant)
            .await
            .unwrap()
            .expect("only an attached public variant can be published");
        assert!(url.starts_with("https://cdn.example/public/covers/950/1/"));
        assert!(url.ends_with(".webp"));
        let key = url.strip_prefix("https://cdn.example/").unwrap();
        public_urls.push((variant, url.clone()));
        let head = client
            .head_object()
            .bucket(PUBLIC_BUCKET)
            .key(key)
            .send()
            .await
            .unwrap();
        assert_eq!(head.content_type(), Some("image/webp"));
        assert_eq!(
            head.cache_control(),
            Some("public, max-age=60, must-revalidate")
        );
    }
    assert!(ledger.publication_candidates(3).await.unwrap().is_empty());
    for kind in [PublicCoverKind::Card, PublicCoverKind::Main] {
        let urls = direct
            .current_cdn_urls(&[BOOK_ID as u64, 951], kind)
            .await
            .expect("current copied public BASIC cover only");
        assert_eq!(urls.len(), 1);
        let expected_url = public_urls
            .iter()
            .find(|(variant, _)| *variant == kind)
            .map(|(_, url)| url)
            .unwrap();
        assert_eq!(urls.get(&(BOOK_ID as u64)), Some(expected_url));
        assert!(!urls.contains_key(&951));
    }
    cdn_html::verify(&catalog, &publisher, &public_urls).await;

    let card_original = store
        .get_bounded(&card.storage_key, card.size_bytes as usize)
        .await
        .unwrap();
    let response = request(&app, "/covers/950/card.webp").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/webp");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(response.headers().get("location").is_none());
    let public_bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(digest(&public_bytes), card.sha256);
    assert_eq!(public_bytes.as_ref(), card_original.as_slice());
    assert_eq!(
        request(&app, "/covers/950/main.webp").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "/covers/951/card.webp").await.status(),
        StatusCode::NOT_FOUND
    );

    // An old same-origin cover URL must stop resolving when its public Book is
    // pending deletion, and both list projection and both variants must agree.
    // Existing private S3 variants stay stored; only public access is revoked.
    let previous_lifecycle = pg_sql(
        &url,
        &format!("SELECT lifecycle_status FROM books WHERE id={BOOK_ID}"),
    )
    .await;
    assert!(
        ["in_progress", "completed", "abandoned", "on_hiatus"]
            .contains(&previous_lifecycle.as_str()),
        "expected a public, non-deleting fixture"
    );
    let initial_cards = slots.public_card_ids(&[BOOK_ID as u64]).await.unwrap();
    assert!(initial_cards.contains(&(BOOK_ID as u64)));
    pg_sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='pending_deletion' WHERE id={BOOK_ID}"),
    )
    .await;
    assert!(
        direct
            .current_cdn_urls(&[BOOK_ID as u64], PublicCoverKind::Card)
            .await
            .unwrap()
            .is_empty(),
        "hidden books must no longer expose permanent CDN URLs"
    );
    // A bounded origin-cleanup worker claims two durable PG records, removes
    // exact public S3 keys, and leaves private originals untouched.
    let cleanup = CoverOriginCleanup::new(ledger.clone(), Arc::new(publisher.clone()));
    let clean = cleanup.run_batch().await.unwrap();
    assert_eq!(clean.claimed, 2);
    assert_eq!(clean.origin_deleted, 2);
    assert_eq!(clean.retry_scheduled, 0);
    assert_eq!(cleanup.run_batch().await.unwrap().claimed, 0);
    for (kind, public_url) in &public_urls {
        let name = match kind {
            PublicCoverKind::Card => "card",
            PublicCoverKind::Main => "main",
        };
        let image = stored_variants
            .iter()
            .find(|item| item.name == name)
            .unwrap();
        let key = public_url.strip_prefix("https://cdn.example/").unwrap();
        assert!(client
            .head_object()
            .bucket(PUBLIC_BUCKET)
            .key(key)
            .send()
            .await
            .is_err());
        assert!(store
            .exists_verified(&image.storage_key, &image.sha256, image.size_bytes)
            .await
            .unwrap());
    }
    // Exact CDN purge submission and polling stay separate from origin deletion.
    #[cfg(not(feature = "test-cdn-loopback"))]
    let fake_cdn = Arc::new(cdn_fake::FakeCoverCdn::default());
    #[cfg(feature = "test-cdn-loopback")]
    let (endpoint, http_mock) = cdn_wire::start(&public_urls).await;
    #[cfg(feature = "test-cdn-loopback")]
    let api: Arc<dyn CoverCdnApi> = Arc::new(
        YandexCdnPurge::new("res-1234".into(), "mock-secret-123".into())
            .unwrap()
            .with_loopback_endpoint(&endpoint)
            .unwrap(),
    );
    #[cfg(not(feature = "test-cdn-loopback"))]
    let api: Arc<dyn CoverCdnApi> = fake_cdn.clone();
    let purge = CoverCdnPurgeWorker::new(ledger.clone(), api);
    let submitted = purge.submit_batch().await.unwrap();
    assert_eq!(submitted.claimed, 2);
    assert_eq!(submitted.submitted, 2);
    assert_eq!(purge.submit_batch().await.unwrap().claimed, 0);
    #[cfg(not(feature = "test-cdn-loopback"))]
    assert_eq!(fake_cdn.submissions(), 2);

    // Force only local CI retry timestamps forward; real worker waits.
    pg_sql(
        &url,
        "UPDATE public_cover_publications SET retry_after=now()
         WHERE book_id=950 AND state='purge_submitted'",
    )
    .await;
    let pending = purge.poll_batch().await.unwrap();
    assert_eq!(pending.claimed, 2);
    assert_eq!(pending.completed, 0);
    assert_eq!(pending.deferred, 2);
    for id in &publication_ids {
        let state = pg_sql(
            &url,
            &format!("SELECT state FROM public_cover_publications WHERE id={id}"),
        )
        .await;
        assert_eq!(
            state, "purge_submitted",
            "pending CDN operation is not purged"
        );
    }
    pg_sql(
        &url,
        "UPDATE public_cover_publications SET retry_after=now()
         WHERE book_id=950 AND state='purge_submitted'",
    )
    .await;
    // One operation fails at the provider, the other succeeds. The failed
    // item must return to origin_deleted, never be acknowledged as purged.
    let partial = purge.poll_batch().await.unwrap();
    assert_eq!(partial.completed, 1);
    assert_eq!(partial.retry_scheduled, 1);
    let failed_count = pg_sql(
        &url,
        "SELECT count(*) FROM public_cover_publications
         WHERE book_id=950 AND state='origin_deleted'",
    )
    .await;
    assert_eq!(failed_count, "1");
    // Accelerate the retry timestamp only inside this disposable CI database.
    pg_sql(
        &url,
        "UPDATE public_cover_publications SET retry_after=now()
         WHERE book_id=950 AND state='origin_deleted'",
    )
    .await;
    let retry = purge.submit_batch().await.unwrap();
    assert_eq!(retry.claimed, 1);
    assert_eq!(retry.submitted, 1);
    #[cfg(not(feature = "test-cdn-loopback"))]
    assert_eq!(fake_cdn.submissions(), 3);
    pg_sql(
        &url,
        "UPDATE public_cover_publications SET retry_after=now()
         WHERE book_id=950 AND state='purge_submitted'",
    )
    .await;
    let last = purge.poll_batch().await.unwrap();
    assert_eq!(last.completed, 1);
    for id in publication_ids {
        let state = pg_sql(
            &url,
            &format!("SELECT state FROM public_cover_publications WHERE id={id}"),
        )
        .await;
        assert_eq!(state, "purged");
    }
    assert_eq!(purge.poll_batch().await.unwrap().claimed, 0);
    #[cfg(feature = "test-cdn-loopback")]
    http_mock.await.unwrap();
    assert!(
        direct
            .publish_cdn_variant(BOOK_ID as u64, PublicCoverKind::Card)
            .await
            .unwrap()
            .is_none(),
        "a hidden book cannot publish further public CDN copies"
    );
    assert!(
        slots
            .public_card_ids(&[BOOK_ID as u64])
            .await
            .unwrap()
            .is_empty(),
        "a deleted Book cannot appear in homepage/catalog cover projections"
    );
    assert!(slots
        .public_variant(BOOK_ID, PublicCoverKind::Card)
        .await
        .unwrap()
        .is_none());
    for kind in ["card", "main"] {
        assert_eq!(
            request(&app, &format!("/covers/{BOOK_ID}/{kind}.webp"))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "previous cover URL must be revoked"
        );
    }
    pg_sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='{previous_lifecycle}' WHERE id={BOOK_ID}"),
    )
    .await;
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::OK,
        "restoring a public Book reuses the same validated cover"
    );
    assert_eq!(
        slots.current_slot(BOOK_ID).await.unwrap(),
        Some((job, 1)),
        "lifecycle revocation does not publish a new cover revision"
    );

    // A malicious provider writer can forge HEAD metadata. The public HTTP
    // adapter still checks GET bytes and refuses to leak corrupted images.
    let mut damaged = card_original.clone();
    *damaged.last_mut().unwrap() ^= 0x7f;
    let claimed_hash: String = card
        .sha256
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    client
        .put_object()
        .bucket(BUCKET)
        .key(&card.storage_key)
        .content_type("image/webp")
        .metadata("sha256", claimed_hash)
        .body(ByteStream::from(damaged))
        .send()
        .await
        .unwrap();
    assert!(store
        .exists_verified(&card.storage_key, &card.sha256, card.size_bytes)
        .await
        .unwrap());
    let corrupted = request(&app, "/covers/950/card.webp").await;
    assert_eq!(corrupted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(to_bytes(corrupted.into_body(), 1024)
        .await
        .unwrap()
        .is_empty());

    store
        .put_verified(&card.storage_key, &card_original)
        .await
        .expect("signed repair of a corrupted private S3 object");
    assert_eq!(
        request(&app, "/covers/950/card.webp").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        slots.current_slot(BOOK_ID).await.unwrap(),
        Some((job, 1)),
        "recovering a stored object must not attach another revision"
    );
    catalog.pool().close().await;
}
