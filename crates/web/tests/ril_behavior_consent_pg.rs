//! Current-schema RIL: old flags and legacy writers cannot identify navigation.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::num::NonZeroU32;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn fixture_sql(url: &str, statement: &str) -> String {
    let target = reqwest::Url::parse(url).expect("fixture database URL");
    assert!(matches!(target.scheme(), "postgres" | "postgresql"));
    assert_eq!(target.host_str(), Some("127.0.0.1"));
    assert_eq!(target.port(), Some(5432));
    assert_eq!(target.path(), "/tor_ci");
    assert!(target.query().is_none() && target.fragment().is_none());
    let output = std::process::Command::new("psql")
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
            statement,
        ])
        .output()
        .expect("FLOW public fixture");
    assert!(
        output.status.success(),
        "FLOW fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    cookie: &str,
    origin: &str,
) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
#[ignore = "isolated PG18 with retirement 192 and enabled legacy RIL flags required"]
async fn retired_consent_and_legacy_flags_never_identify_navigation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test URL");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    assert_eq!(std::env::var("TOR_RIL_SINGLE_INSTANCE").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("TOR_FLOW_PUBLIC_OPT_IN_ENABLED").as_deref(),
        Ok("1")
    );
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let current_schema = fixture_sql(
        &url,
        "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations
                       WHERE version=202611168903 AND success)
            AND EXISTS(SELECT 1 FROM pg_trigger
                       WHERE tgrelid='ril_behavior_consents'::regclass
                         AND tgname='retired_collection_write_fence' AND tgenabled<>'D')",
    );
    assert_eq!(
        current_schema, "t",
        "current-schema RIL proof requires retirement 192"
    );
    let reader = PgReader::new(catalog.pool().clone());
    let accounts = PgAccounts::from_pool(catalog.pool());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user = accounts
        .insert_user(
            &format!("ril-retired-{stamp}@example.invalid"),
            "RIL Test",
            &hash_password("unused-test-passphrase").unwrap(),
        )
        .await
        .unwrap();
    // Real published pages, not just accepted route templates.
    let book: i64 = fixture_sql(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level) \
             VALUES('ci:flow:public:{stamp}','ru','FLOW public {stamp}','public',{user},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = fixture_sql(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) \
             VALUES({book},1,'FLOW public chapter','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = fixture_sql(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({chapter},1,'A published public FLOW chapter.') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    fixture_sql(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    let book_path = format!("/books/{book}");
    let reader_path = format!("/reader/{chapter}");

    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (false, 0)
    );
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    accounts
        .insert_session(user, &digest, NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    let session = format!("tor-dev-session={token}");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );

    let first = send(&app, "GET", "/me", "", &session, ORIGIN).await;
    assert_eq!(first.status(), StatusCode::OK);
    let csrf_cookie = first
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("tor-dev-csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html =
        String::from_utf8(to_bytes(first.into_body(), 100_000).await.unwrap().to_vec()).unwrap();
    // No separate consent UI remains on the ordinary account page.
    assert!(!html.contains("Анализ ваших переходов"));
    assert!(!html.contains("Приватность"));
    let csrf = html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let cookies = format!("{session}; {csrf_cookie}");
    // A valid account cookie never enables the retired identity observer.
    assert_eq!(
        send(&app, "GET", "/", "", &cookies, ORIGIN).await.status(),
        StatusCode::OK
    );
    for surface in [book_path.as_str(), reader_path.as_str()] {
        assert_eq!(
            send(&app, "GET", surface, "", &cookies, ORIGIN)
                .await
                .status(),
            StatusCode::OK,
            "{surface}"
        );
    }

    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .assessed_events,
        0
    );
    let enable = format!("choice=enable&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, "", ORIGIN)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // A valid CSRF token without an authenticated session is not consent.
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, &csrf_cookie, ORIGIN)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            "/me/behavior",
            &enable,
            &cookies,
            "https://invalid.example"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let retired = send(&app, "POST", "/me/behavior", &enable, &cookies, ORIGIN).await;
    assert_eq!(retired.status(), StatusCode::GONE);
    assert_eq!(retired.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(retired.headers()["x-robots-tag"], "noindex, nofollow");
    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (false, 0),
        "retired HTTP writes no consent row"
    );
    // An old process cannot re-enable identity after the SQL fence.
    // Preservation of real pre-fence rows is proved by the installed-upgrade suite.
    let rejected = accounts
        .set_ril_behavior_consent(user, true)
        .await
        .unwrap_err();
    assert_eq!(
        rejected
            .as_database_error()
            .expect("retirement must reject the legacy SQL writer")
            .code()
            .as_deref(),
        Some("55000")
    );
    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (false, 0)
    );
    assert_eq!(
        fixture_sql(
            &url,
            &format!("SELECT count(*) FROM ril_behavior_consents WHERE user_id={user}")
        ),
        "0",
        "the rejected legacy writer must not leave a disabled consent row"
    );
    for _ in 0..3 {
        assert_eq!(
            send(&app, "GET", "/me", "", &cookies, ORIGIN)
                .await
                .status(),
            StatusCode::OK
        );
    }
    let report = tor_web::ril_path_report_for_diagnostics().unwrap();
    assert_eq!(report.assessed_events, 0);
    assert_eq!(report.active_subjects, 0);
    // The rejected opt-in cannot produce a subject or path queue.
    for surface in ["/", "/catalog", book_path.as_str(), reader_path.as_str()] {
        let response = send(&app, "GET", surface, "", &cookies, ORIGIN).await;
        assert_eq!(response.status(), StatusCode::OK, "{surface}");
        assert!(response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
    }
    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .assessed_events,
        0
    );

    // Guests can read the same pages without becoming identified visitors.
    for surface in [book_path.as_str(), reader_path.as_str()] {
        assert_eq!(
            send(&app, "GET", surface, "", "", ORIGIN).await.status(),
            StatusCode::OK,
            "{surface}"
        );
    }

    let disable = format!("choice=disable&csrf_token={csrf}");
    let retired = send(&app, "POST", "/me/behavior", &disable, &cookies, ORIGIN).await;
    assert_eq!(retired.status(), StatusCode::GONE);
    assert_eq!(retired.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(retired.headers()["x-robots-tag"], "noindex, nofollow");
    assert_eq!(
        accounts.ril_behavior_consent(user).await.unwrap(),
        (false, 0),
        "the retired HTTP command cannot change consent or its epoch"
    );
    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .active_subjects,
        0
    );
    assert_eq!(
        send(&app, "GET", "/me", "", &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::OK
    );
    for surface in ["/catalog", book_path.as_str(), reader_path.as_str()] {
        assert_eq!(
            send(&app, "GET", surface, "", &cookies, ORIGIN)
                .await
                .status(),
            StatusCode::OK,
            "{surface}"
        );
    }
    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .assessed_events,
        0
    );

    assert_eq!(
        fixture_sql(
            &url,
            &format!("SELECT count(*) FROM ril_behavior_consents WHERE user_id={user}")
        ),
        "0",
        "navigation and both retired commands leave no consent row"
    );
    accounts.revoke_session(&digest).await.unwrap();
    // A revoked login cookie must not restore identified public navigation.
    assert_eq!(
        send(&app, "GET", &reader_path, "", &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        tor_web::ril_path_report_for_diagnostics()
            .unwrap()
            .assessed_events,
        0
    );
    assert_eq!(
        send(&app, "POST", "/me/behavior", &enable, &cookies, ORIGIN)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
