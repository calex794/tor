//! Owner upload -> real PG queue -> G image worker -> public NEwBOO cover.
use super::*;
use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Cursor,
    sync::{Arc, Mutex},
};
use tor_books::repositories::cover_slots::PgCoverSlots;
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
    let slots = PgCoverSlots::new(pool);
    let bridge = PgBookCoverBridge::new(slots.clone());
    let worker = CoverWorker {
        jobs: &jobs,
        storage: store,
        books: &bridge,
    };
    for _ in 0..16 {
        if slots.current_slot(book_id).await.unwrap().is_some() {
            break;
        }
        match worker.run_one().await.unwrap() {
            WorkerResult::Idle => panic!("queued image was never processed"),
            WorkerResult::Attached | WorkerResult::Superseded | WorkerResult::RetryOrQuarantine => {}
        }
    }
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
    assert!(slots.current_slot(book_id).await.unwrap().is_some());

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
        body_of(send(app, request("GET", "/catalog?author=Alice", "", "", ORIGIN)).await).await;
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
    assert!(owned.contains("Обложка готова."));
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
}
