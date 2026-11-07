//! S026 end-to-end: Team creates a private owner-owned entity, not Book ACL.
//! Reuse the existing registered S033 PostgreSQL HTTP test target.
use super::{body, cookie, csrf_from_page, send, signup, ORIGIN};
use axum::http::{header, StatusCode};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::repositories::PgAccounts;
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
    let csrf = csrf_from_page(&body(page).await);
    let csrf_page = send(&app, "GET", "/me/teams", &owner, "").await;
    let csrf_cookie = cookie(&csrf_page, "tor-dev-csrf=");
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
    let actor: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email_normalized=$1")
        .bind(&owner_email)
        .fetch_one(&pool)
        .await
        .unwrap();
    let team = PgTranslationTeams::new(pool.clone())
        .owned(actor, id)
        .await
        .unwrap();
    assert_eq!(team.owner_user_id, actor);
    assert_eq!(team.name, "Team<script>");
    let number: i64 =
        sqlx::query_scalar("SELECT count(*) FROM translation_teams WHERE owner_user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(number, 1, "duplicate did not create a second Team");
    let book_grants: i64 = sqlx::query_scalar("SELECT count(*) FROM book_members WHERE user_id=$1")
        .bind(actor)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_grants, 0, "Team does not authorize access to any Book");
    let paid_grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(paid_grants, 0, "Team does not grant purchased chapters");

    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, "GET", &route, &cookies, "").await.status(),
        StatusCode::SEE_OTHER,
        "suspended creator's revoked session cannot read Team"
    );
    // No production migrations, external HTTP or Book permissions were mutated.
}
