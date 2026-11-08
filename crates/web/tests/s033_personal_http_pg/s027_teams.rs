//! S027 Team invite HTTP + isolated PostgreSQL acceptance; never production.
use super::{body, cookie, csrf_from_page, send, signup, ORIGIN};
use axum::http::{header, StatusCode};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;

fn token_from_html(html: &str) -> String {
    html.split("/me/teams/invites/")
        .nth(1)
        .expect("one-time invite link")
        .split('"')
        .next()
        .expect("link token")
        .to_owned()
}

#[tokio::test]
#[ignore = "requires disposable migrated PostgreSQL tor_ci"]
async fn s027_recipient_only_invites_idempotence_and_no_book_permissions() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test PG");
    assert!(url.ends_with("/tor_ci"), "never write to retained PG");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let recipient_email = format!("s027-recipient-{stamp}@example.invalid");
    let owner = signup(&app, &format!("s027-owner-{stamp}@example.invalid")).await;
    let recipient = signup(&app, &recipient_email).await;
    let stranger = signup(&app, &format!("s027-stranger-{stamp}@example.invalid")).await;
    let owner_page = send(&app, "GET", "/me/teams", &owner, "").await;
    let csrf_cookie = cookie(&owner_page, "tor-dev-csrf=");
    let csrf = csrf_from_page(&body(owner_page).await);
    let owner_cookies = format!("{owner}; {csrf_cookie}");
    let team = send(
        &app,
        "POST",
        "/me/teams",
        &owner_cookies,
        &format!("name=InvitationTest{stamp}&description=Team&csrf_token={csrf}"),
    )
    .await;
    assert_eq!(team.status(), StatusCode::SEE_OTHER);
    let team_path = team.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let detail = body(send(&app, "GET", &team_path, &owner_cookies, "").await).await;
    assert!(detail.contains("recipient_email"));
    let invite_path = format!("{team_path}/invites");
    let email = recipient_email.replace('@', "%40");
    let invite_form = format!("recipient_email={email}&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &invite_path, &owner, &invite_form)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "CSRF cookie is required"
    );
    assert_eq!(
        send(&app, "POST", &invite_path, &stranger, &invite_form)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "stranger must not create invite"
    );
    let create = send(&app, "POST", &invite_path, &owner_cookies, &invite_form).await;
    assert_eq!(create.status(), StatusCode::OK);
    assert_eq!(create.headers()[header::CACHE_CONTROL], "no-store");
    let token = token_from_html(&body(create).await);
    assert_eq!(token.len(), 43);
    let invite_url = format!("/me/teams/invites/{token}");
    assert_eq!(
        send(&app, "GET", &invite_url, &stranger, "").await.status(),
        StatusCode::NOT_FOUND,
        "bearer token alone cannot open another user's invite"
    );
    assert_eq!(
        send(&app, "GET", &invite_url, &owner, "").await.status(),
        StatusCode::NOT_FOUND,
        "inviter is not the invitee"
    );
    let shown = send(&app, "GET", &invite_url, &recipient, "").await;
    assert_eq!(shown.status(), StatusCode::OK);
    let recipient_csrf = cookie(&shown, "tor-dev-csrf=");
    let recipient_token = csrf_from_page(&body(shown).await);
    let recipient_cookies = format!("{recipient}; {recipient_csrf}");
    let accept_path = format!("{invite_url}/accept");
    let accept_form = format!("csrf_token={recipient_token}");
    assert_eq!(
        send(&app, "POST", &accept_path, &stranger, &accept_form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &accept_path, &recipient, &accept_form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &accept_path, &recipient_cookies, &accept_form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &accept_path, &recipient_cookies, &accept_form)
            .await
            .status(),
        StatusCode::SEE_OTHER,
        "accept must be idempotent"
    );
    let team_html = body(send(&app, "GET", &team_path, &recipient, "").await).await;
    assert!(team_html.contains("Вы участник команды"));
    assert!(
        !team_html.contains("recipient_email"),
        "members cannot invite"
    );
    let list = body(send(&app, "GET", "/me/teams", &recipient, "").await).await;
    assert!(list.contains(&team_path));
    let member_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM translation_team_members WHERE team_id=$1")
            .bind(
                team_path
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .parse::<i64>()
                    .unwrap(),
            )
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(member_count, 1);
    assert_eq!(
        send(&app, "GET", &team_path, &stranger, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        !body(send(&app, "GET", "/me/books", &recipient, "").await)
            .await
            .contains("InvitationTest"),
        "Team membership grants no Book ownership or paid entitlement"
    );
    assert_eq!(
        send(&app,"POST",&invite_path,&owner_cookies,&invite_form).await.status(),
        StatusCode::CONFLICT, "a member cannot be invited again"
    );
}
