//! H-09/S023: real owner HTTP -> PostgreSQL file import, no background jobs.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::authoring::new_book,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn call(
    app: &Router,
    path: &str,
    method: &str,
    cookies: &str,
    origin: &str,
    json: Option<Value>,
) -> (StatusCode, String, Option<String>) {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookies);
    if method == "POST" {
        request = request
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/json");
    }
    let body = json.map_or_else(Body::empty, |value| Body::from(value.to_string()));
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let text = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, text, csp)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn h_09_owner_preview_and_confirm_are_private_and_replay_safe() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "no production database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let author = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("H09ChapterImport2026").unwrap();
    let owner_token = generate_session_token();
    let other_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("h09owner-{nonce}@tor-ci.example"),
            "Author",
            &password,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let _other = accounts
        .register_with_session(
            &format!("h09other-{nonce}@tor-ci.example"),
            "Stranger",
            &password,
            &session_token_digest(&other_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book = new_book(
        &format!("tor-ci:import-http:{nonce}"),
        "ru",
        "Черновик для импорта",
        &"a".repeat(43),
    )
    .unwrap();
    let id = author.create_owned_book(owner, &book).await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let other_cookie = format!("tor-dev-session={other_token}; tor-dev-csrf={csrf}");
    let root = format!("/me/books/{id}/chapters/import");
    let preview_url = format!("{root}-preview");
    let confirm_url = format!("{root}-confirm");
    assert_eq!(
        call(&app, &root, "GET", "", ORIGIN, None).await.0,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        call(&app, &root, "GET", &other_cookie, ORIGIN, None)
            .await
            .0,
        StatusCode::NOT_FOUND,
    );
    let page = call(&app, &root, "GET", &owner_cookie, ORIGIN, None).await;
    assert_eq!(page.0, StatusCode::OK);
    assert!(page.1.contains("Импорт глав из файла"));
    assert!(page.1.contains("/assets/newboo-chapter-import.js"));
    assert!(page.1.contains(&format!("value=\"{csrf}\"")));
    assert!(page.2.unwrap().contains("script-src 'self'"));
    let script = call(
        &app,
        "/assets/newboo-chapter-import.js",
        "GET",
        "",
        ORIGIN,
        None,
    )
    .await;
    assert_eq!(script.0, StatusCode::OK);
    assert!(script.1.contains("TextDecoder"));

    let source = "Глава 1: Начало\nПервый текст\nГлава 2: Продолжение\nВторой текст";
    let key = generate_session_token();
    let preview_json = json!({
        "filename": "batch.txt", "text": source, "csrf_token": csrf,
    });
    assert_eq!(
        call(
            &app,
            &preview_url,
            "POST",
            &owner_cookie,
            "https://attacker.invalid",
            Some(preview_json.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(
            &app,
            &preview_url,
            "POST",
            &owner_cookie,
            ORIGIN,
            Some(json!({"filename":"batch.txt","text":source,"csrf_token":"wrong"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(
            &app,
            &preview_url,
            "POST",
            &other_cookie,
            ORIGIN,
            Some(preview_json.clone())
        )
        .await
        .0,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        call(
            &app,
            &preview_url,
            "POST",
            "",
            ORIGIN,
            Some(preview_json.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(
            &app,
            &preview_url,
            "POST",
            &owner_cookie,
            ORIGIN,
            Some(json!({"filename":"batch.zip","text":source,"csrf_token":csrf}))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(
        call(
            &app,
            &preview_url,
            "POST",
            &owner_cookie,
            ORIGIN,
            Some(json!({"filename":"batch.txt",
                "text":"Глава 1: Начало\nТекст\nГлава 1: Повтор\nТекст",
                "csrf_token":csrf}))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY,
    );

    let preview = call(
        &app,
        &preview_url,
        "POST",
        &owner_cookie,
        ORIGIN,
        Some(preview_json.clone()),
    )
    .await;
    assert_eq!(preview.0, StatusCode::OK, "{}", preview.1);
    let p: Value = serde_json::from_str(&preview.1).unwrap();
    assert_eq!(p["count"], 2);
    assert_eq!(p["chapters"][0]["number"], 1);
    let fingerprint = p["fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 64);
    let mut confirm = preview_json.as_object().unwrap().clone();
    confirm.insert("command_key".into(), json!(key));
    confirm.insert("fingerprint".into(), json!("tampered"));
    assert_eq!(
        call(
            &app,
            &confirm_url,
            "POST",
            &owner_cookie,
            ORIGIN,
            Some(Value::Object(confirm.clone()))
        )
        .await
        .0,
        StatusCode::CONFLICT,
    );
    confirm.insert("fingerprint".into(), json!(fingerprint));
    let saved = call(
        &app,
        &confirm_url,
        "POST",
        &owner_cookie,
        ORIGIN,
        Some(Value::Object(confirm.clone())),
    )
    .await;
    assert_eq!(saved.0, StatusCode::OK, "{}", saved.1);
    assert_eq!(serde_json::from_str::<Value>(&saved.1).unwrap()["saved"], 2);
    let replay = call(
        &app,
        &confirm_url,
        "POST",
        &owner_cookie,
        ORIGIN,
        Some(Value::Object(confirm.clone())),
    )
    .await;
    assert_eq!(replay.0, StatusCode::OK);

    let saved_chapters = author.list_owned_chapters(owner, id).await.unwrap();
    assert_eq!(saved_chapters.len(), 2);
    assert_eq!(saved_chapters[0].number, 1);
    assert_eq!(saved_chapters[1].number, 2);
    // The B-owned PostgreSQL test separately verifies no first-release event.
    // This HTTP test also proves a guest still receives 404 on the book.
    assert_eq!(
        call(&app, &format!("/books/{id}"), "GET", "", ORIGIN, None)
            .await
            .0,
        StatusCode::NOT_FOUND,
    );
    // A different command with a conflicting chapter number cannot leave
    // partially written chapters behind. The repository test covers 2 items.
    let another = json!({
        "filename":"batch.txt",
        "text":"Глава 1: Повтор\nДругой текст",
        "csrf_token":csrf,
    });
    let changed_preview = call(
        &app,
        &preview_url,
        "POST",
        &owner_cookie,
        ORIGIN,
        Some(another.clone()),
    )
    .await;
    assert_eq!(changed_preview.0, StatusCode::OK);
    let mut altered = another.as_object().unwrap().clone();
    altered.insert("command_key".into(), json!(key));
    altered.insert(
        "fingerprint".into(),
        json!(serde_json::from_str::<Value>(&changed_preview.1).unwrap()["fingerprint"]),
    );
    assert_eq!(
        call(
            &app,
            &confirm_url,
            "POST",
            &owner_cookie,
            ORIGIN,
            Some(Value::Object(altered))
        )
        .await
        .0,
        StatusCode::CONFLICT,
    );
    pool.close().await;
}
