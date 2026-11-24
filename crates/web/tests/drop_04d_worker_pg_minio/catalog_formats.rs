//! FND-03 / S023: authenticated preview, durable queue and bodyless publication for each source format.
use super::{insert_book, list_all_keys, published_editions, query_json, verify_edition, Fixture};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Cursor, Write},
    num::NonZeroU32,
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::{repositories::PgCatalog, services::chapter_file_import::ImportedChapter};
use tor_imports::services::ChapterImportObjectStore;
use tor_reading::repositories::PgReader;
use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;
use tor_web::chapter_import_worker::ChapterImportWorkerResult;
use tower::ServiceExt;
use zip::{write::SimpleFileOptions, ZipWriter};

const BUCKET: &str = "tor-fnd03-source-formats-ci";
const ORIGIN: &str = "http://127.0.0.1:3098";

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and loopback MinIO"]
async fn fnd03_supported_uploads_publish_exact_s3_editions() {
    let fixture = Fixture::new_in_bucket("FND-03 TXT", BUCKET).await;
    let pool = fixture.catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let token = generate_session_token();
    let csrf = generate_session_token();
    accounts
        .insert_session(
            fixture.actor,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let app = tor_web::router_with_postgres_reading_auth(
        PgCatalog::connect(&fixture.url).await.unwrap(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.into(),
    )
    .merge(tor_web::chapter_import_queue_http::router(
        accounts,
        false,
        ORIGIN.into(),
        fixture.source.clone(),
    ));
    let title = "Глава 1: Начало";
    let body = "Кириллица, 中文 и 😀.";
    let text = format!("{title}\n{body}");
    let markdown = format!("# {text}");
    let document = format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{title}</w:t></w:r></w:p><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"#
    );
    let sources = [
        ("plain.txt", "plain.txt", json!({"text": text})),
        ("chapter.md", "chapter.md", json!({"text": markdown})),
        (
            "word.docx",
            "word.txt",
            json!({
                "text": "",
                "file_base64": archive(&[("[Content_Types].xml", "<Types/>"), ("word/document.xml", &document)])
            }),
        ),
        (
            "collection.zip",
            "collection.txt",
            json!({"text": "", "file_base64": archive(&[("chapter.md", &markdown)]), "archive_encoding": "utf-8"}),
        ),
    ];
    let mut imported = Vec::new();
    for (index, (original, queued, mut preview)) in sources.into_iter().enumerate() {
        let book = if index == 0 {
            fixture.book
        } else {
            insert_book(&fixture.url, fixture.actor, original)
        };
        let route = format!("/me/books/{book}/chapters/import");
        preview["filename"] = json!(original);
        preview["csrf_token"] = json!(csrf);
        let result = post(
            &app,
            &format!("{route}-preview"),
            &cookie,
            &preview,
            StatusCode::OK,
        )
        .await;
        assert_eq!(result["count"], 1);
        let normalized = if original == queued {
            assert!(result.get("extracted_text").is_none());
            preview["text"].as_str().unwrap()
        } else {
            result["extracted_text"].as_str().unwrap()
        };
        let queue_route = format!("{route}-queue");
        let command = json!({
            "filename": queued,
            "text": normalized,
            "csrf_token": csrf,
            "command_key": generate_session_token(),
            "fingerprint": result["fingerprint"],
        });
        if original == "collection.zip" {
            let mut changed = command.clone();
            changed["text"] = json!(format!("{normalized}\nНесогласованный текст"));
            post(&app, &queue_route, &cookie, &changed, StatusCode::CONFLICT).await;
            let jobs: i64 = query_json(
                &fixture,
                &format!("SELECT count(*) FROM chapter_import_jobs WHERE book_id={book}"),
            );
            assert_eq!(jobs, 0);
        }
        let accepted = post(&app, &queue_route, &cookie, &command, StatusCode::OK).await;
        assert_eq!(accepted["status"], "queued");
        let job = accepted["job_id"].as_i64().unwrap();
        let (filename, key): (String, String) = query_json(
            &fixture,
            &format!("SELECT json_build_array(source_filename,source_key) FROM chapter_import_jobs WHERE id={job}"),
        );
        assert_eq!(filename, queued);
        let hash = Sha256::digest(normalized.as_bytes()).into();
        assert_eq!(
            fixture
                .source
                .get_verified(&key, normalized.len() as i64, &hash, normalized.len())
                .await
                .unwrap()
                .as_slice(),
            normalized.as_bytes()
        );
        assert!(published_editions(&fixture, book).is_empty());
        imported.push((book, job));
    }

    let worker = fixture.worker(false);
    for _ in &imported {
        assert_eq!(
            worker.run_one().await.unwrap(),
            ChapterImportWorkerResult::Completed
        );
    }
    assert_eq!(
        worker.run_one().await.unwrap(),
        ChapterImportWorkerResult::Idle
    );
    let expected = ImportedChapter {
        number: 1,
        title: title.into(),
        body: body.into(),
    };
    let store = S3FreeChapterStore::new(fixture.client.clone(), BUCKET.into()).unwrap();
    let mut keys = BTreeSet::new();
    for (book, job) in imported {
        let stored: (String, i32) = query_json(
            &fixture,
            &format!("SELECT json_build_array(status,saved_count) FROM chapter_import_jobs WHERE id={job}"),
        );
        assert_eq!(stored, ("succeeded".into(), 1));
        let editions = published_editions(&fixture, book);
        assert_eq!(editions.len(), 1);
        assert!(keys.insert(verify_edition(&store, BUCKET, &editions[0], &expected).await));
    }
    assert_eq!(list_all_keys(&fixture.client, BUCKET).await, keys);
    println!("FND03_SUPPORTED_UPLOADS_OK formats={}", keys.len());
}

fn archive(entries: &[(&str, &str)]) -> String {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (filename, content) in entries {
        zip.start_file(*filename, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    STANDARD.encode(zip.finish().unwrap().into_inner())
}

async fn post(
    app: &Router,
    path: &str,
    cookie: &str,
    input: &Value,
    expected_status: StatusCode,
) -> Value {
    let request = Request::post(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(input.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        expected_status,
        "failed request to {path}"
    );
    let bytes = to_bytes(response.into_body(), 16_384).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
