//! S028 Team Book link and member removal HTTP coverage on disposable PostgreSQL.
use super::{body, cookie, csrf_from_page, send, signup, ORIGIN};
use axum::http::{header, StatusCode};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::{
    repositories::{
        book_members::PgBookMembers, postgres::PgAuthoring, translation_team_invites::PgTeamInvites,
        PgCatalog,
    },
    services::authoring::new_book,
};
use tor_contracts::access::BookRole;
use tor_reading::repositories::PgReader;

#[tokio::test]
#[ignore = "isolated, migrated PostgreSQL tor_ci"]
async fn s028_team_book_association_requires_two_owners_and_member_removal_never_grants_book_access() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test database");
    assert!(url.ends_with("/tor_ci"), "never write outside disposable PG");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let lookup = accounts.clone();
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
    let owner = signup(&app, &format!("s028-owner-{stamp}@example.invalid")).await;
    let member_email = format!("s028-member-{stamp}@example.invalid");
    let member = signup(&app, &member_email).await;
    let outsider = signup(&app, &format!("s028-outsider-{stamp}@example.invalid")).await;
    let id = |session: &str| session_token_digest(session.split_once('=').unwrap().1);
    let owner_id = lookup.active_session_user(&id(&owner)).await.unwrap().unwrap();
    let member_id = lookup.active_session_user(&id(&member)).await.unwrap().unwrap();
    let owner_start = send(&app, "GET", "/me/teams", &owner, "").await;
    let owner_csrf_cookie = cookie(&owner_start, "tor-dev-csrf=");
    let csrf = csrf_from_page(&body(owner_start).await);
    let owner_cookies = format!("{owner}; {owner_csrf_cookie}");
    let created = send(
        &app,
        "POST",
        "/me/teams",
        &owner_cookies,
        &format!("name=TeamBooks{stamp}&csrf_token={csrf}"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let team_path = created.headers()[header::LOCATION].to_str().unwrap().to_owned();
    let team_id = team_path.rsplit('/').next().unwrap().parse::<i64>().unwrap();
    let authoring = PgAuthoring::new(pool.clone());
    let owner_book = new_book(
        &format!("s028:owner:{stamp}"),
        "ru",
        &format!("Owner Book {stamp}"),
        &generate_session_token(),
    )
    .unwrap();
    let owner_book_id = authoring.create_owned_book(owner_id, &owner_book).await.unwrap();
    let member_book = new_book(
        &format!("s028:member:{stamp}"),
        "ru",
        &format!("Member Book {stamp}"),
        &generate_session_token(),
    )
    .unwrap();
    let member_book_id = authoring.create_owned_book(member_id, &member_book).await.unwrap();
    let book_members = PgBookMembers::new(pool.clone());
    assert_eq!(book_members.active_role(owner_id, owner_book_id).await.unwrap(), Some(BookRole::Owner));
    assert_eq!(book_members.active_role(member_id, owner_book_id).await.unwrap(), None);
    let token = generate_session_token();
    let invites = PgTeamInvites::new(pool.clone());
    invites.invite(owner_id, team_id, &member_email, &token).await.unwrap();
    invites.decide(member_id, &token, true).await.unwrap();

    let book_path = format!("{team_path}/books");
    assert_eq!(send(&app, "GET", &book_path, &outsider, "").await.status(), StatusCode::NOT_FOUND);
    let member_page = send(&app, "GET", &book_path, &member, "").await;
    assert_eq!(member_page.status(), StatusCode::OK);
    let member_cookie = cookie(&member_page, "tor-dev-csrf=");
    let member_token = csrf_from_page(&body(member_page).await);
    let member_cookies = format!("{member}; {member_cookie}");
    let foreign_form = format!("book_id={owner_book_id}&csrf_token={member_token}");
    assert_eq!(
        send(&app, "POST", &book_path, &member_cookies, &foreign_form).await.status(),
        StatusCode::NOT_FOUND,
        "Team membership cannot attach a Book owned by someone else"
    );
    let own_form = format!("book_id={member_book_id}&csrf_token={member_token}");
    assert_eq!(
        send(&app, "POST", &book_path, &member, &own_form).await.status(),
        StatusCode::FORBIDDEN,
        "CSRF cookie required"
    );
    assert_eq!(
        send(&app, "POST", &book_path, &member_cookies, &own_form).await.status(),
        StatusCode::SEE_OTHER
    );
    let owner_form = format!("book_id={owner_book_id}&csrf_token={csrf}");
    assert_eq!(
        send(&app, "POST", &book_path, &owner_cookies, &owner_form).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &book_path, &owner_cookies, &owner_form).await.status(),
        StatusCode::CONFLICT
    );
    let member_view = body(send(&app, "GET", &book_path, &member, "").await).await;
    assert!(member_view.contains(&format!("Owner Book {stamp}")));
    assert!(member_view.contains(&format!("Member Book {stamp}")));
    let remove_book = format!("{book_path}/{member_book_id}/remove");
    assert_eq!(
        send(&app, "POST", &remove_book, &member_cookies, &format!("csrf_token={member_token}")).await.status(),
        StatusCode::NOT_FOUND,
        "member cannot remove associated book"
    );
    assert_eq!(
        send(&app, "POST", &remove_book, &owner_cookies, &format!("csrf_token={csrf}")).await.status(),
        StatusCode::SEE_OTHER
    );
    assert!(!body(send(&app, "GET", &book_path, &owner, "").await).await.contains(&format!("Member Book {stamp}")));

    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{team_path}/members/{member_id}/remove"),
            &member_cookies,
            &format!("csrf_token={member_token}")
        ).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("{team_path}/members/{member_id}/remove"),
            &owner_cookies,
            &format!("csrf_token={csrf}")
        ).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(send(&app, "GET", &book_path, &member, "").await.status(), StatusCode::NOT_FOUND);
    assert_eq!(book_members.active_role(owner_id, owner_book_id).await.unwrap(), Some(BookRole::Owner));
    assert_eq!(book_members.active_role(member_id, member_book_id).await.unwrap(), Some(BookRole::Owner));
    assert_eq!(book_members.active_role(member_id, owner_book_id).await.unwrap(), None);
    assert_eq!(send(&app, "GET", &book_path, "", "").await.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        send(&app, "GET", &book_path, &owner, "").await.headers()[header::CACHE_CONTROL],
        "no-store"
    );
}
