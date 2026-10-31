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
    let own_page = text(call(&app, "GET", &path, &cookies[0], "", &[]).await).await;
    assert!(own_page.contains("Team &lt;script&gt;"));
    assert!(!own_page.contains("Team <script>"));
    assert!(own_page.contains("Создатель группы пока не может выйти"));
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
    let peer_page = text(call(&app, "GET", &path, &cookies[1], "", &[]).await).await;
    assert!(peer_page.contains("&lt;img src=x onerror=alert(1)&gt;"));
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
