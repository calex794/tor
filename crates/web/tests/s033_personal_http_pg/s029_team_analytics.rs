//! S029 Team analytics HTML and CSV share the same Team/book filters.
use super::{body, cookie, csrf_from_page, send, signup, ORIGIN};
use axum::http::{header, StatusCode};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::services::security::generate_session_token;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{
    repositories::{postgres::PgAuthoring, translation_team_invites::PgTeamInvites, PgCatalog},
    services::authoring::new_book,
};
use tor_reading::repositories::{
    book_stats::PgBookStats, PgChapterThanks, PgReader, ThanksMutation,
};

/// Insert only synthetic, bounded fixtures into tor_ci, matching the established
/// Book stats HTTP regression. No production DB or persistent migrations.
fn pg(url: &str, statement: &str) -> String {
    let result = std::process::Command::new("psql")
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
        .expect("psql");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
#[ignore = "disposable migrated PostgreSQL tor_ci"]
async fn s029_analytics_csv_matches_ui_book_period_scope_without_foreign_book_data() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test database");
    assert!(url.ends_with("/tor_ci"), "never write to permanent PG");
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
    let owner = signup(&app, &format!("s029-owner-{stamp}@example.invalid")).await;
    let foreign = signup(&app, &format!("s029-foreign-{stamp}@example.invalid")).await;
    let member_email = format!("s029-member-{stamp}@example.invalid");
    let member = signup(&app, &member_email).await;
    let site_moderator = signup(
        &app,
        &format!("s029-site-moderator-{stamp}@example.invalid"),
    )
    .await;
    let owner_id = lookup
        .active_session_user(&session_token_digest(owner.split_once('=').unwrap().1))
        .await
        .unwrap()
        .unwrap();
    let foreign_id = lookup
        .active_session_user(&session_token_digest(foreign.split_once('=').unwrap().1))
        .await
        .unwrap()
        .unwrap();
    let member_id = lookup
        .active_session_user(&session_token_digest(member.split_once('=').unwrap().1))
        .await
        .unwrap()
        .unwrap();
    // Synthetic role bootstrap is confined to disposable tor_ci; the approval
    // itself goes through the real reviewed moderation HTTP endpoint.
    let moderator_id = lookup
        .active_session_user(&session_token_digest(
            site_moderator.split_once('=').unwrap().1,
        ))
        .await
        .unwrap()
        .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({moderator_id},'moderator',TRUE)"
        ),
    );
    let reviewer_page = send(&app, "GET", "/me/teams", &site_moderator, "").await;
    let reviewer_cookie = cookie(&reviewer_page, "tor-dev-csrf=");
    let reviewer_csrf = csrf_from_page(&body(reviewer_page).await);
    let reviewer_cookies = format!("{site_moderator}; {reviewer_cookie}");
    let team_page = send(&app, "GET", "/me/teams", &owner, "").await;
    let cookie_csrf = cookie(&team_page, "tor-dev-csrf=");
    let token = csrf_from_page(&body(team_page).await);
    let owner_cookies = format!("{owner}; {cookie_csrf}");
    let created = send(
        &app,
        "POST",
        "/me/teams",
        &owner_cookies,
        &format!("name=Stats{stamp}&csrf_token={token}"),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let team = created.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let team_id = team.rsplit('/').next().unwrap().parse::<i64>().unwrap();
    let invitation = generate_session_token();
    let invites = PgTeamInvites::new(pool.clone());
    invites
        .invite(owner_id, team_id, &member_email, &invitation)
        .await
        .unwrap();
    invites.decide(member_id, &invitation, true).await.unwrap();
    let authoring = PgAuthoring::new(pool.clone());
    let mine = new_book(
        &format!("s029:owner:{stamp}"),
        "ru",
        &format!("A Book {stamp}"),
        &generate_session_token(),
    )
    .unwrap();
    let own_book_id = authoring.create_owned_book(owner_id, &mine).await.unwrap();
    let theirs = new_book(
        &format!("s029:foreign:{stamp}"),
        "ru",
        &format!("Z Book {stamp}"),
        &generate_session_token(),
    )
    .unwrap();
    let other_book_id = authoring
        .create_owned_book(foreign_id, &theirs)
        .await
        .unwrap();
    let linked = send(
        &app,
        "POST",
        &format!("{team}/books"),
        &owner_cookies,
        &format!("book_id={own_book_id}&csrf_token={token}"),
    )
    .await;
    assert_eq!(linked.status(), StatusCode::SEE_OTHER);

    // Seed a genuinely published public free chapter and record actual views.
    // This proves the analytics model reads nonzero counters rather than defaults.
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({own_book_id},1,'Analytics free','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Analytics reader fixture') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    authoring.release_book(owner_id, own_book_id).await.unwrap();
    assert_eq!(
        send(
            &app,
            "POST",
            &format!("/moderation/books/{own_book_id}/registration"),
            &reviewer_cookies,
            &format!("csrf_token={reviewer_csrf}&expected_revision=1&action=approve&confirmed=yes"),
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "registration moderation is required before a fresh Book can go public"
    );
    let stats = PgBookStats::new(pool.clone());
    stats.record_open(chapter, Some(foreign_id)).await.unwrap();
    stats.record_open(chapter, None).await.unwrap();
    let thanks = PgChapterThanks::new(pool.clone());
    assert_eq!(
        thanks
            .thank_visible_free(foreign_id, chapter)
            .await
            .unwrap(),
        ThanksMutation::Added
    );
    let scoped = format!("{team}/analytics?period=7d&book_id={own_book_id}");
    let html = body(send(&app, "GET", &scoped, &owner, "").await).await;
    assert!(html.contains(&format!("A Book {stamp}")));
    assert!(!html.contains(&format!("Z Book {stamp}")));
    assert!(html.contains("7 дней"));
    let csv_path = format!("{team}/analytics.csv?period=7d&book_id={own_book_id}");
    let csv_response = send(&app, "GET", &csv_path, &owner, "").await;
    assert_eq!(csv_response.status(), StatusCode::OK);
    assert_eq!(
        csv_response.headers()[header::CONTENT_TYPE],
        "text/csv; charset=utf-8"
    );
    assert_eq!(csv_response.headers()[header::CACHE_CONTROL], "no-store");
    let csv = body(csv_response).await;
    assert!(csv.contains(&format!("A Book {stamp}")));
    assert!(csv.contains(&format!(";{own_book_id};")));
    assert!(!csv.contains(&format!("Z Book {stamp}")));
    assert!(!csv.contains(&format!(";{other_book_id};")));
    assert_eq!(csv.lines().count(), 3, "header, total, one linked book");
    let csv_book = csv.lines().nth(2).unwrap();
    let values = csv_book
        .trim_end_matches('\r')
        .split(';')
        .collect::<Vec<_>>();
    assert_eq!(values[5], "2", "actual public chapter opens");
    assert_eq!(values[6], "1", "anonymous opening");
    assert_eq!(values[7], "1", "first authenticated chapter opening");
    assert_eq!(values[8], "1", "new registered reader of this book");
    assert_eq!(values[9], "0", "no fabricated purchases");
    assert_eq!(values[11], "1", "one real Thanks event");
    assert_eq!(
        send(&app, "GET", &scoped, &member, "").await.status(),
        StatusCode::OK,
        "invited active team member has Team analytics without Book ACL"
    );
    assert_eq!(
        send(&app, "GET", &csv_path, &member, "").await.status(),
        StatusCode::OK
    );
    for route in [&scoped, &csv_path] {
        assert_eq!(
            send(&app, "GET", route, &foreign, "").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    for route in [
        format!("{team}/analytics?period=30d&book_id={other_book_id}"),
        format!("{team}/analytics.csv?period=30d&book_id={other_book_id}"),
    ] {
        assert_eq!(
            send(&app, "GET", &route, &owner, "").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{team}/analytics?period=200d"),
            &owner,
            ""
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, "GET", &format!("{team}/analytics.csv"), "", "")
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}
