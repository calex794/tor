//! E-08: real group-mail forms on isolated PostgreSQL.
use super::{call, pg, text, ORIGIN};
use axum::http::{header, StatusCode};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_groups_create_read_send_replay_leave_and_csrf() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = hash_password("GroupMailTestingPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for i in 0..4 {
        let session = generate_session_token();
        let csrf = generate_session_token();
        let user = accounts
            .register_with_session(
                &format!("group-http-{nonce}-{i}@example.invalid"),
                &format!("Group reader {i}"),
                &hash,
                &session_token_digest(&session),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        users.push(user);
        cookies.push(format!("tor-dev-session={session}; tor-dev-csrf={csrf}"));
        tokens.push(csrf);
    }
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    assert_eq!(
        call(&app, "GET", "/mail/groups", "", "", &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let initial = call(&app, "GET", "/mail/groups", &cookies[0], "", &[]).await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert!(initial.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'none'"));
    assert!(text(initial)
        .await
        .contains("Групповых переписок пока нет."));

    for peer in [users[1], users[2]] {
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("/mail/start/{peer}"),
                &cookies[0],
                &format!("csrf_token={}", tokens[0]),
                &[],
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    }
    let index = text(call(&app, "GET", "/mail/groups", &cookies[0], "", &[]).await).await;
    assert!(index.contains("Group reader 1"));
    assert!(index.contains("Group reader 2"));
    assert!(index.contains("name=\"peer_one\""));
    assert!(index.contains("name=\"peer_two\""));
    let form = format!(
        "title=Team%20%3Cscript%3E&peer_one={}&peer_two={}&csrf_token={}",
        users[1], users[2], tokens[0]
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/mail/groups",
            &cookies[0],
            &form,
            &[("origin", "https://evil.invalid")]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/mail/groups",
            &cookies[0],
            &form.replace(&tokens[0], "forged"),
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/mail/groups",
            &cookies[0],
            &format!(
                "title=Bad&peer_one={}&peer_two={}&csrf_token={}",
                users[1], users[1], tokens[0]
            ),
            &[]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // The pinned NEwBOO start_group_conversation accepts the creator plus
    // exactly one invited member. An empty optional selector must be valid.
    let pair_form = format!(
        "title=Two%20readers&peer_one={}&peer_two=&csrf_token={}",
        users[1], tokens[0]
    );
    let pair_created = call(&app, "POST", "/mail/groups", &cookies[0], &pair_form, &[]).await;
    assert_eq!(pair_created.status(), StatusCode::SEE_OTHER);
    let pair_path = pair_created.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        call(&app, "GET", &pair_path, &cookies[1], "", &[])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &pair_path, &cookies[2], "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let missing_second = format!(
        "title=Also%20two&peer_one={}&csrf_token={}",
        users[1], tokens[0]
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/mail/groups",
            &cookies[0],
            &missing_second,
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let invalid_second = format!(
        "title=Invalid&peer_one={}&peer_two=not-a-number&csrf_token={}",
        users[1], tokens[0]
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/mail/groups",
            &cookies[0],
            &invalid_second,
            &[]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let created = call(&app, "POST", "/mail/groups", &cookies[0], &form, &[]).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let path = created.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(path.starts_with("/mail/groups/"));
    let group_id: i64 = path.trim_start_matches("/mail/groups/").parse().unwrap();
    assert_eq!(
        call(&app, "GET", &path, &cookies[3], "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/messages"),
            &cookies[3],
            &format!(
                "body=forged&command_key={}&csrf_token={}",
                generate_session_token(),
                tokens[3]
            ),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    // Group live polling uses same-origin fetch() and must be permitted by
    // connect-src. The index remains script-free and cannot make connections.
    let own_response = call(&app, "GET", &path, &cookies[0], "", &[]).await;
    assert_eq!(own_response.status(), StatusCode::OK);
    let group_csp = own_response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(group_csp.contains("script-src 'self'"));
    assert!(group_csp.contains("connect-src 'self'"));
    assert!(group_csp.contains("default-src 'none'"));
    let own_page = text(own_response).await;
    assert!(own_page.contains(r#"<script defer src="/assets/mail.js"></script>"#));
    assert!(own_page.contains("Team &lt;script&gt;"));
    assert!(!own_page.contains("Team <script>"));
    assert!(own_page.contains("Создатель группы пока не может выйти"));
    assert!(own_page.contains("Участники (3)"));
    assert!(own_page.contains("Group reader 1"));
    assert!(own_page.contains("Group reader 2"));
    let key = own_page
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(key.len(), 43);
    let send = format!(
        "body=%3Cimg%20src%3Dx%20onerror%3Dalert(1)%3E&command_key={key}&csrf_token={}",
        tokens[0]
    );
    for _ in 0..2 {
        assert_eq!(
            call(
                &app,
                "POST",
                &format!("{path}/messages"),
                &cookies[0],
                &send,
                &[]
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_group_messages WHERE group_id={group_id}")
        )
        .await,
        "1"
    );
    // GET polling is the same private membership boundary as HTML and POST.
    let poll_path = format!("{path}/messages");
    assert_eq!(
        call(&app, "GET", &poll_path, "", "", &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", &poll_path, &cookies[3], "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = call(
        &app,
        "GET",
        &format!("{poll_path}?after_id=0&limit=50"),
        &cookies[1],
        "",
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let polled: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(polled["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        polled["messages"][0]["text"],
        "<img src=x onerror=alert(1)>"
    );
    assert_eq!(polled["messages"][0]["sender_name"], "Group reader 0");
    let id = polled["messages"][0]["id"].as_str().unwrap();
    assert!(id.parse::<i64>().unwrap() > 0);
    let response = call(
        &app,
        "GET",
        &format!("{poll_path}?after_id={id}"),
        &cookies[1],
        "",
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let empty: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert!(empty["messages"].as_array().unwrap().is_empty());
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{poll_path}?before_id=1&after_id=0"),
            &cookies[0],
            "",
            &[],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{poll_path}?actor_id={}", users[0]),
            &cookies[1],
            "",
            &[],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let peer_page = text(call(&app, "GET", &path, &cookies[1], "", &[]).await).await;
    assert!(peer_page.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(peer_page.contains("<strong>Group reader 0</strong>"));
    assert!(!peer_page.contains("<img src=x onerror=alert(1)>"));
    assert!(peer_page.contains("Выйти из группы"));
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/leave"),
            &cookies[1],
            &format!("csrf_token={}", tokens[1]),
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "GET", &path, &cookies[1], "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}/messages"),
            &cookies[1],
            "",
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/messages"),
            &cookies[1],
            &format!(
                "body=after&command_key={}&csrf_token={}",
                generate_session_token(),
                tokens[1]
            ),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/leave"),
            &cookies[0],
            &format!("csrf_token={}", tokens[0]),
            &[]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "GET", &path, &cookies[2], "", &[])
            .await
            .status(),
        StatusCode::OK
    );
    // All fixtures live in tor_ci, which is discarded after this run.
}
