//! H-09/S023: real owner HTTP -> PostgreSQL file import, no background jobs.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    num::NonZeroU32,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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


/// P1/Argon: actual authenticated TXT and ZIP preview requests against the
/// production Axum router. Two held PostgreSQL connections simulate a busy
/// three-connection pool; they are NOT a substitute for live S3 workers.
/// This exercise must never target a retained or production database.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "isolated PostgreSQL 18 /tor_ci; concurrent author imports and simulated pool pressure"]
async fn h_09_cpu_import_keeps_http_responsive_under_pool_pressure() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never run against a retained database");

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let author = PgAuthoring::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("H09CPUContention2026").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("h09-cpu-{nonce}@tor-ci.example"),
            "CPU Author",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book = new_book(
        &format!("tor-ci:cpu-contention:{nonce}"),
        "ru",
        "Конкурентный импорт",
        &"a".repeat(43),
    )
    .unwrap();
    let book_id = author.create_owned_book(owner, &book).await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let preview_path = format!("/me/books/{book_id}/chapters/import-preview");

    // A large but valid multi-chapter TXT and a valid STORED ZIP of the same
    // material: ZIP extraction reparses the archive and checks every member.
    // STORED compression avoids tripping the intentional zip-bomb ratio guard.
    let text = (1..=400)
        .map(|number| format!("Глава {number}: Нагрузка\n{}\n", "x".repeat(1300)))
        .collect::<String>();
    assert!(text.len() > 500_000 && text.len() < 700_000);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "chapters.txt",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(text.as_bytes()).unwrap();
    let archive = writer.finish().unwrap().into_inner();
    let text_input = json!({
        "filename": "chapters.txt",
        "text": text,
        "csrf_token": csrf.clone(),
    });
    let zip_input = json!({
        "filename": "chapters.zip",
        "text": "",
        "file_base64": base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &archive,
        ),
        "archive_encoding": "utf-8",
        "csrf_token": csrf,
    });

    const REQUESTS: usize = 18;
    let gate = Arc::new(tokio::sync::Barrier::new(REQUESTS + 2));
    let mut requests = tokio::task::JoinSet::new();
    for index in 0..REQUESTS {
        let app = app.clone();
        let path = preview_path.clone();
        let cookie = cookie.clone();
        let input = if index % 2 == 0 {
            text_input.clone()
        } else {
            zip_input.clone()
        };
        let gate = Arc::clone(&gate);
        requests.spawn(async move {
            gate.wait().await;
            let started = Instant::now();
            let (status, body, _) = call(&app, &path, "POST", &cookie, ORIGIN, Some(input)).await;
            (status, body, started.elapsed())
        });
    }

    // Simulate two background DB consumers holding connections. The same PgPool
    // serves author authentication, the readiness probe and other SQL traffic.
    let pressure_pool = pool.clone();
    let pressure_gate = Arc::clone(&gate);
    let simulated_db_consumers = tokio::spawn(async move {
        pressure_gate.wait().await;
        for _ in 0..6 {
            let first = pressure_pool.acquire().await.expect("first connection");
            let second = pressure_pool.acquire().await.expect("second connection");
            tokio::time::sleep(Duration::from_millis(35)).await;
            drop((first, second));
            tokio::task::yield_now().await;
        }
    });

    let probe_app = app.clone();
    let probe_gate = Arc::clone(&gate);
    let probes = tokio::spawn(async move {
        probe_gate.wait().await;
        let mut health_samples = Vec::with_capacity(80);
        let mut ready_samples = Vec::with_capacity(16);
        for sample in 0..80 {
            let started = Instant::now();
            let (status, _, _) = call(&probe_app, "/healthz", "GET", "", ORIGIN, None).await;
            assert_eq!(status, StatusCode::OK, "event-loop liveness");
            health_samples.push(started.elapsed());

            if sample % 5 == 0 {
                let started = Instant::now();
                let (status, _, _) = call(&probe_app, "/readyz", "GET", "", ORIGIN, None).await;
                assert_eq!(status, StatusCode::OK, "shared PostgreSQL readiness");
                ready_samples.push(started.elapsed());
            }
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        (health_samples, ready_samples)
    });

    let mut ok = 0_usize;
    let mut busy = 0_usize;
    let mut import_samples = Vec::with_capacity(REQUESTS);
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(joined) = requests.join_next().await {
            let (status, response, elapsed) = joined.expect("import task joined");
            import_samples.push(elapsed);
            match status {
                StatusCode::OK => {
                    let preview: Value = serde_json::from_str(&response).unwrap();
                    assert_eq!(preview["count"], 400);
                    assert_eq!(preview["fingerprint"].as_str().unwrap().len(), 64);
                    ok += 1;
                }
                StatusCode::TOO_MANY_REQUESTS => busy += 1,
                unexpected => {
                    panic!("unexpected import status {unexpected}: {response}");
                }
            }
        }
    })
    .await
    .expect("bounded imports must not hang");

    simulated_db_consumers.await.expect("simulated DB consumers");
    let (mut health, mut ready) = probes.await.expect("HTTP liveness probes");
    health.sort_unstable();
    ready.sort_unstable();
    import_samples.sort_unstable();
    let health_p95 = health[health.len() * 95 / 100];
    let health_p99 = health[health.len() * 99 / 100];
    let ready_p95 = ready[ready.len() * 95 / 100];
    let import_p95 = import_samples[import_samples.len() * 95 / 100];
    println!(
        "TOR_AUTHOR_CPU_CONTENTION requests={REQUESTS} imported_previews={ok} backpressure_429={busy} health_p95_ms={} health_p99_ms={} ready_p95_ms={} import_p95_ms={}",
        health_p95.as_millis(),
        health_p99.as_millis(),
        ready_p95.as_millis(),
        import_p95.as_millis()
    );
    assert_eq!(ok + busy, REQUESTS);
    assert!(ok > 0, "at least one complete import preview must succeed");
    assert!(
        health_p95 < Duration::from_millis(250),
        "CPU pressure must not starve ordinary HTTP on two Tokio workers"
    );
    assert!(
        health_p99 < Duration::from_secs(1),
        "p99 event loop responsiveness regressed"
    );
    assert!(
        ready_p95 < Duration::from_secs(2),
        "three-connection shared pool became unresponsive"
    );
    assert!(
        author.list_owned_chapters(owner, book_id).await.unwrap().is_empty(),
        "previews must not write chapters"
    );
    pool.close().await;
}
