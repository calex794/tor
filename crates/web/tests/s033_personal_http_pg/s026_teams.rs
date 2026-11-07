//! S026 end-to-end: Team creates a private owner-owned entity, not Book ACL.
//! Reuse the existing registered S033 PostgreSQL HTTP test target.
use super::{body, cookie, csrf_from_page, send, signup, ORIGIN};
use axum::http::{header, StatusCode};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{repositories::translation_teams::PgTranslationTeams, repositories::PgCatalog};
use tor_reading::repositories::PgReader;

#[tokio::test]
#[ignore = "disposable, migrated PostgreSQL tor_ci and normal HTTP account flows"]
async fn s026_team_creation_is_owner_only_and_never_grants_book_rights() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never write to retained database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let verified_accounts = accounts.clone();
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
    let owner_email = format!("s026-owner-{stamp}@example.invalid");
    let owner = signup(&app, &owner_email).await;
    let visitor = signup(&app, &format!("s026-other-{stamp}@example.invalid")).await;

    let anonymous = send(&app, "GET", "/me/teams", "", "").await;
    assert_eq!(anonymous.status(), StatusCode::SEE_OTHER);
    assert_eq!(anonymous.headers()[header::CACHE_CONTROL], "no-store");
    let page = send(&app, "GET", "/me/teams", &owner, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers()["x-robots-tag"],
        "noindex, nofollow, noarchive"
    );
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let csrf = csrf_from_page(&body(page).await);
    let cookies = format!("{owner}; {csrf_cookie}");

    let request = format!("name=Team%3Cscript%3E&description=%3Csvg%3E&csrf_token={csrf}");
    let invalid = send(&app, "POST", "/me/teams", &owner, &request).await;
    assert_eq!(invalid.status(), StatusCode::FORBIDDEN, "no CSRF cookie");

    let created = send(&app, "POST", "/me/teams", &cookies, &request).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let route = created.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(route.starts_with("/me/teams/"));
    let detail = send(&app, "GET", &route, &cookies, "").await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(detail).await;
    assert!(html.contains("Team&lt;script&gt;"));
    assert!(html.contains("&lt;svg&gt;"));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<svg>"));

    assert_eq!(
        send(&app, "GET", &route, &visitor, "").await.status(),
        StatusCode::NOT_FOUND,
        "another authenticated account cannot view the owner's Team"
    );
    assert_eq!(
        send(&app, "GET", &route, "", "").await.status(),
        StatusCode::SEE_OTHER
    );
    let other_list = body(send(&app, "GET", "/me/teams", &visitor, "").await).await;
    assert!(!other_list.contains("Team&lt;script&gt;"));
    assert!(body(send(&app, "GET", "/me/teams", &cookies, "").await)
        .await
        .contains(&route));

    let duplicate = send(
        &app,
        "POST",
        "/me/teams",
        &cookies,
        &format!("name=team%3Cscript%3E&description=x&csrf_token={csrf}"),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(
        send(
            &app,
            "POST",
            "/me/teams",
            &cookies,
            &format!("name=&csrf_token={csrf}"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    let id: i64 = route.rsplit('/').next().unwrap().parse().unwrap();
    let digest = session_token_digest(owner.split_once('=').unwrap().1);
    let actor = verified_accounts
        .active_session_user(&digest)
        .await
        .unwrap()
        .expect("verified owner from active session");
    let repository = PgTranslationTeams::new(pool.clone());
    let team = repository.owned(actor, id).await.unwrap();
    assert_eq!(team.owner_user_id, actor);
    assert_eq!(team.name, "Team<script>");
    assert_eq!(
        repository.list_owned(actor).await.unwrap().len(),
        1,
        "duplicate form did not create another Team"
    );
    assert!(
        !body(send(&app, "GET", "/catalog", "", "").await)
            .await
            .contains("Team&lt;script&gt;"),
        "Team is not a public book entry"
    );
    assert!(
        !body(send(&app, "GET", "/me/books", &owner, "").await)
            .await
            .contains("Team&lt;script&gt;"),
        "Team did not create a Book permission or owner Book"
    );

    verified_accounts.revoke_session(&digest).await.unwrap();
    assert_eq!(
        send(&app, "GET", &route, &cookies, "").await.status(),
        StatusCode::SEE_OTHER,
        "revoked session must not open the Team cabinet"
    );
    // No production migrations, external HTTP or Book permissions were mutated.
}
