//! Owner upload -> real PG queue -> G image worker -> public NEwBOO cover.
use super::*;
use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Cursor,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};
use tor_accounts::services::security::{
    generate_session_token, hash_password, session_token_digest,
};
use tor_books::repositories::{cover_review::PgCoverReview, cover_slots::PgCoverSlots};
use tor_ingest::{
    domain::cover::CoverError,
    repositories::cover_jobs::PgCoverJobs,
    services::{
        cover_jobs::CoverObjectStore,
        cover_worker::{CoverWorker, WorkerResult},
    },
};
use tor_web::cover_worker::PgBookCoverBridge;

#[derive(Clone, Default)]
pub(super) struct MemoryCoverStore {
    objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
#[async_trait]
impl CoverObjectStore for MemoryCoverStore {
    async fn get_bounded(&self, key: &str, max: usize) -> Result<Vec<u8>, CoverError> {
        let bytes = self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(CoverError::Unavailable)?;
        if bytes.len() > max {
            return Err(CoverError::Invalid);
        }
        Ok(bytes)
    }
    async fn put_source_verified(
        &self,
        key: &str,
        bytes: &[u8],
        mime: &str,
        sha: &[u8; 32],
    ) -> Result<(), CoverError> {
        if !key.starts_with("staging/covers/sources/")
            || !matches!(mime, "image/png" | "image/jpeg" | "image/webp")
            || bytes.is_empty()
            || digest(bytes) != *sha
        {
            return Err(CoverError::Invalid);
        }
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }
    async fn put_verified(&self, key: &str, bytes: &[u8]) -> Result<(), CoverError> {
        if !key.starts_with("staging/covers/variants/") {
            return Err(CoverError::Invalid);
        }
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }
    async fn exists_verified(
        &self,
        key: &str,
        sha: &[u8; 32],
        size: i64,
    ) -> Result<bool, CoverError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|bytes| bytes.len() as i64 == size && digest(bytes) == *sha))
    }
}

fn png() -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(80, 120, Rgb([30, 90, 160])));
    let mut result = Cursor::new(Vec::new());
    image.write_to(&mut result, ImageFormat::Png).unwrap();
    result.into_inner()
}

fn upload_request(
    uri: &str,
    csrf: &str,
    cookies: &str,
    origin: &str,
    mime: &str,
    bytes: &[u8],
) -> Request<Body> {
    const SEP: &str = "tor-cover-test-boundary";
    let mut body = format!(
        "--{SEP}\r\nContent-Disposition: form-data; name=\"csrf_token\"\r\n\r\n{csrf}\r\n\
         --{SEP}\r\nContent-Disposition: form-data; name=\"cover\"; filename=\"sample.png\"\r\n\
         Content-Type: {mime}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{SEP}--\r\n").as_bytes());
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={SEP}"),
        )
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(body))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40001))));
    request
}

pub(super) async fn upload_and_show(
    app: &Router,
    store: &MemoryCoverStore,
    book_id: i64,
    cookies: &str,
    csrf: &str,
) {
    let path = format!("/me/books/{book_id}/cover");
    let source = png();
    assert_eq!(
        send(
            app,
            upload_request(&path, csrf, "", ORIGIN, "image/png", &source)
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            app,
            upload_request(
                &path,
                csrf,
                cookies,
                "https://evil.example",
                "image/png",
                &source
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app,
            upload_request(&path, "wrong", cookies, ORIGIN, "image/png", &source)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app,
            upload_request(&path, csrf, cookies, ORIGIN, "image/png", b"<svg>bad</svg>")
        )
        .await
        .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    // The non-owner cannot alter a seeded book, even with their own valid session.
    let denied = "/me/books/901/cover";
    assert_eq!(
        send(
            app,
            upload_request(denied, csrf, cookies, ORIGIN, "image/png", &source)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let posted = send(
        app,
        upload_request(&path, csrf, cookies, ORIGIN, "image/png", &source),
    )
    .await;
    assert_eq!(posted.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        posted.headers()[header::LOCATION],
        format!("/me/books/{book_id}/settings/covers")
    );
    assert_eq!(posted.headers()[header::CACHE_CONTROL], "no-store");
    let settings = send(
        app,
        request(
            "GET",
            &format!("/me/books/{book_id}/settings/covers"),
            "",
            cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(settings.status(), StatusCode::OK);
    let csp = settings.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self'"));
    assert!(csp.contains("img-src 'self' blob:"));
    assert!(csp.contains("frame-ancestors 'none'"));
    let settings_html = body_of(settings).await;
    assert!(settings_html.contains("book-cover-picker__frame"));
    assert!(settings_html.contains("book-cover-picker__plus"));
    assert!(settings_html.contains("Обложка 1"));
    assert!(settings_html.contains("data-cover-picker-input"));
    assert!(settings_html.contains("name=\"cover\""));
    assert!(settings_html.contains("/assets/newboo-cover-picker.js"));
    assert!(!settings_html.contains("cover_file_2"));
    assert!(settings_html.contains("Обложка ожидает обработки."));
    let anonymous_settings = send(
        app,
        request(
            "GET",
            &format!("/me/books/{book_id}/settings/covers"),
            "",
            "",
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(anonymous_settings.status(), StatusCode::SEE_OTHER);
    let page = send(
        app,
        request("GET", &format!("/me/books/{book_id}"), "", cookies, ORIGIN),
    )
    .await;
    assert!(body_of(page).await.contains("Обложка ожидает обработки."));

    // The production worker reads a durable PG lease and writes two real WebP
    // variants, then B atomically attaches the cover to the public book.
    let catalog = PgCatalog::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let pool = catalog.pool();
    let jobs = PgCoverJobs::new(pool.clone());
    let slots = PgCoverSlots::new(pool.clone());
    let reviews = PgCoverReview::new(pool.clone());
    let bridge = PgBookCoverBridge::new(slots.clone());
    let worker = CoverWorker {
        jobs: &jobs,
        storage: store,
        books: &bridge,
    };
    // Real separate active site-moderator session. The owner and worker cannot
    // publish a processed image without an independent approved review.
    let accounts = tor_accounts::repositories::PgAccounts::from_pool(pool.clone());
    let reviewer_token = generate_session_token();
    let reviewer = accounts
        .register_with_session(
            &format!("cover-reviewer-{book_id}@tor-ci.example"),
            "Cover site moderator",
            &hash_password("CoverTestReviewer2026!").unwrap(),
            &session_token_digest(&reviewer_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let role_sql = format!(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({reviewer},'moderator',TRUE)"
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let assigned = std::process::Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url.as_str(),
            "--command",
            role_sql.as_str(),
        ])
        .output()
        .unwrap();
    assert!(
        assigned.status.success(),
        "disposable moderator role fixture failed: {}",
        String::from_utf8_lossy(&assigned.stderr)
    );
    let reviewer_digest = session_token_digest(&reviewer_token);
    let mut candidate = None;
    for _ in 0..16 {
        if let Some(review) = reviews
            .pending(&reviewer_digest, None)
            .await
            .unwrap()
            .into_iter()
            .find(|review| review.book_id == book_id)
        {
            candidate = Some(review);
            break;
        }
        match worker.run_one().await.unwrap() {
            WorkerResult::Idle => panic!("queued image was never processed"),
            WorkerResult::Attached | WorkerResult::Superseded | WorkerResult::RetryOrQuarantine => {
            }
        }
    }
    let candidate = candidate.expect("worker must submit a private candidate");
    assert_eq!(slots.current_slot(book_id).await.unwrap(), None);
    assert_eq!(
        send(
            app,
            request(
                "GET",
                &format!("/covers/{book_id}/main.webp"),
                "",
                "",
                ORIGIN,
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "processed image must stay private until site approval"
    );
    assert_eq!(
        jobs.latest_for_author(book_id, {
            // Only the owner row may provide a status: prove an arbitrary actor cannot see it.
            assert!(jobs
                .latest_for_author(book_id, 99999)
                .await
                .unwrap()
                .is_none());
            let user = tor_accounts::repositories::PgAccounts::from_pool(catalog.pool());
            user.find_user("tor_ci_account@example.com")
                .await
                .unwrap()
                .unwrap()
                .id
        })
        .await
        .unwrap()
        .as_deref(),
        Some("done")
    );
    let queue = send(
        app,
        request(
            "GET",
            "/moderation/covers",
            "",
            &format!("tor-dev-session={reviewer_token}"),
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(queue.status(), StatusCode::OK);
    assert_eq!(
        queue.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let reviewer_csrf_cookie = cookie(&queue, "tor-dev-csrf=");
    let queue_body = body_of(queue).await;
    assert!(queue_body.contains(&format!("/moderation/covers/{}/decision", candidate.id)));
    let reviewer_csrf = csrf_token(&queue_body).to_owned();
    let reviewer_cookie = format!("tor-dev-session={reviewer_token}; {reviewer_csrf_cookie}");
    let decision_path = format!("/moderation/covers/{}/decision", candidate.id);
    let first = send(
        app,
        request(
            "POST",
            &decision_path,
            &format!("csrf_token={reviewer_csrf}&expected_slot_revision=0&action=approve"),
            &reviewer_cookie,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert!(body_of(first)
        .await
        .contains("Подтверждение решения по обложке"));
    assert_eq!(
        slots.current_slot(book_id).await.unwrap(),
        None,
        "first review step must not publish an unapproved image"
    );
    let decision = send(
        app,
        request(
            "POST",
            &decision_path,
            &format!(
                "csrf_token={reviewer_csrf}&expected_slot_revision=0&action=approve&confirmed=yes"
            ),
            &reviewer_cookie,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(decision.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        slots.current_slot(book_id).await.unwrap(),
        Some((candidate.job_id, 1)),
        "only the audited moderator decision may attach the public cover"
    );

    let pub_page = send(
        app,
        request("GET", &format!("/books/{book_id}"), "", "", ORIGIN),
    )
    .await;
    let html = body_of(pub_page).await;
    assert!(html.contains(&format!("src=\"/covers/{book_id}/main.webp\"")));
    let home = body_of(send(app, request("GET", "/", "", "", ORIGIN)).await).await;
    assert!(home.contains(&format!("src=\"/covers/{book_id}/card.webp\"")));
    let catalog_html =
        body_of(send(app, request("GET", "/catalog?q=Alice", "", "", ORIGIN)).await).await;
    assert!(catalog_html.contains(&format!("src=\"/covers/{book_id}/card.webp\"")));
    for variant in ["card.webp", "main.webp"] {
        let response = send(
            app,
            request(
                "GET",
                &format!("/covers/{book_id}/{variant}"),
                "",
                "",
                ORIGIN,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/webp");
    }
    let owned = body_of(
        send(
            app,
            request("GET", &format!("/me/books/{book_id}"), "", cookies, ORIGIN),
        )
        .await,
    )
    .await;
    assert!(
        owned.contains("Обложка одобрена модератором."),
        "owner must see the independently approved cover, not just a processed job"
    );
    let settings = send(
        app,
        request(
            "GET",
            &format!("/me/books/{book_id}/settings/covers"),
            "",
            cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(settings.status(), StatusCode::OK);
    assert!(body_of(settings)
        .await
        .contains(&format!("src=\"/covers/{book_id}/main.webp\"")));

    // MOD-COVER: a site moderator can remove an already approved cover,
    // without unpublishing its Book or deleting immutable private objects.
    let remove_path = format!("/moderation/books/{book_id}/cover-withdraw");
    let history_path = format!("/moderation/books/{book_id}/cover-history");
    let private_history = send(app, request("GET", &history_path, "", &reviewer_cookie, ORIGIN)).await;
    assert_eq!(private_history.status(), StatusCode::OK);
    let history_body = body_of(private_history).await;
    assert!(history_body.contains("Снять действующую обложку"));
    assert!(history_body.contains("expected_slot_revision"));
    assert_eq!(
        send(app, request("POST", &remove_path,
            &format!("csrf_token={csrf}&expected_slot_revision=1&reason=Policy"),
            cookies, ORIGIN)).await.status(),
        StatusCode::FORBIDDEN,
        "Book owner is not a site moderator"
    );
    assert_eq!(
        send(app, request("POST", &remove_path,
            &format!("csrf_token={reviewer_csrf}&expected_slot_revision=1&reason="),
            &reviewer_cookie, ORIGIN)).await.status(),
        StatusCode::BAD_REQUEST
    );
    let confirmation = send(app, request("POST", &remove_path,
        &format!("csrf_token={reviewer_csrf}&expected_slot_revision=1&reason=Policy"),
        &reviewer_cookie, ORIGIN)).await;
    assert_eq!(confirmation.status(), StatusCode::OK);
    assert!(body_of(confirmation).await.contains("Снять действующую обложку"));
    assert_eq!(slots.current_slot(book_id).await.unwrap(), Some((candidate.job_id, 1)));
    let confirmed = format!(
        "csrf_token={reviewer_csrf}&expected_slot_revision=1&action=withdraw&reason=Policy&confirmed=yes"
    );
    assert_eq!(
        send(app, request("POST", &remove_path, &confirmed,
            &reviewer_cookie, ORIGIN)).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(slots.current_slot(book_id).await.unwrap(), Some((candidate.job_id, 2)));
    assert_eq!(reviews.current_active(&reviewer_digest, book_id).await.unwrap(), None);
    for kind in ["card.webp", "main.webp"] {
        assert_eq!(
            send(app, request("GET", &format!("/covers/{book_id}/{kind}"),
                "", "", ORIGIN)).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let public_after = send(app, request("GET", &format!("/books/{book_id}"),
        "", "", ORIGIN)).await;
    assert_eq!(public_after.status(), StatusCode::OK);
    assert!(!body_of(public_after).await.contains(&format!(
        r#"src="/covers/{book_id}/main.webp""#
    )));
    let history_after = body_of(send(app, request("GET", &history_path,
        "", &reviewer_cookie, ORIGIN)).await).await;
    assert!(history_after.contains("withdrawn"));
    assert!(history_after.contains("Действующая обложка отсутствует"));
    assert_eq!(
        send(app, request("POST", &remove_path, &confirmed,
            &reviewer_cookie, ORIGIN)).await.status(),
        StatusCode::CONFLICT,
        "stale/duplicate moderator withdrawal cannot revoke a later cover"
    );
    let all_reviews = reviews.private_history(&reviewer_digest, book_id).await.unwrap();
    assert_eq!(all_reviews.len(), 2);
    assert!(all_reviews.iter().any(|item| item.outcome == "approved"));
    assert!(all_reviews.iter().any(|item| item.outcome == "withdrawn"));
}
