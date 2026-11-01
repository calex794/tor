//! Real administrator group/book mail read-only HTTP tests on disposable PG.
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
use tor_social::repositories::{book_rooms::PgBookRooms, group_mail::PgGroupMail};

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_admin_reads_group_and_book_rooms_without_membership_and_audits() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = hash_password("AdminRoomReadingPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    for i in 0..3 {
        let session = generate_session_token();
        let csrf = generate_session_token();
        let id = accounts
            .register_with_session(
                &format!("e08-admin-room-{nonce}-{i}@example.invalid"),
                &format!("Chat reader {i}"),
                &hash,
                &session_token_digest(&session),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        users.push(id);
        cookies.push(format!("tor-dev-session={session}; tor-dev-csrf={csrf}"));
    }
    let group = PgGroupMail::new(pool.clone());
    let group_id = group
        .create(users[1], &[users[2]], "<script>Team</script>")
        .await
        .unwrap();
    group
        .send(
            users[1],
            group_id,
            "<img src=x onerror=alert(1)>",
            &format!("{:043}", 783),
        )
        .await
        .unwrap();
    let book_id: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:e08:admin-room:{nonce}','ru','<b>Library room</b>','public')
             RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let rooms = PgBookRooms::new(pool.clone());
    rooms.join(users[1], book_id).await.unwrap();
    rooms
        .send(
            users[1],
            book_id,
            "<svg onload=alert(1)>",
            &format!("{:043}", 784),
        )
        .await
        .unwrap();
    // Readers can no longer see the hidden Book room; the administrator
    // must still be able to review its existing historical messages.
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book_id}"),
    )
    .await;
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let groups_path = "/admin/mail/groups";
    let group_path = format!("{groups_path}/{group_id}");
    let rooms_path = "/admin/mail/book-rooms";
    let room_path = format!("{rooms_path}/{book_id}");
    for path in [groups_path, &group_path, rooms_path, &room_path] {
        assert_eq!(
            call(&app, "GET", path, "", "", &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, "GET", path, &cookies[1], "", &[]).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&app, "POST", path, &cookies[1], "", &[])
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
    let base_audit = format!(
        "SELECT count(*) FROM social_mail_admin_access_audit
         WHERE admin_user_id={}",
        users[0]
    );
    assert_eq!(pg(&url, &base_audit).await, "0");
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({},'administrator',TRUE)",
            users[0]
        ),
    )
    .await;
    let list = call(&app, "GET", groups_path, &cookies[0], "", &[]).await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(list.headers()[header::CACHE_CONTROL], "no-store");
    assert!(list.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'none'"));
    let html = text(list).await;
    assert!(html.contains(&group_path));
    assert!(html.contains("&lt;script&gt;Team&lt;/script&gt;"));
    assert!(!html.contains("<script>Team</script>"));
    let list = call(&app, "GET", rooms_path, &cookies[0], "", &[]).await;
    assert_eq!(list.status(), StatusCode::OK);
    let html = text(list).await;
    assert!(html.contains(&room_path));
    assert!(html.contains("&lt;b&gt;Library room&lt;/b&gt;"));
    let msg = call(&app, "GET", &group_path, &cookies[0], "", &[]).await;
    assert_eq!(msg.status(), StatusCode::OK);
    let html = text(msg).await;
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(!html.contains("<img src=x onerror=alert(1)>"));
    assert!(!html.contains("name=\"csrf_token\""));
    assert!(!html.contains("command_key"));
    let msg = call(&app, "GET", &room_path, &cookies[0], "", &[]).await;
    assert_eq!(msg.status(), StatusCode::OK);
    let html = text(msg).await;
    assert!(html.contains("&lt;svg onload=alert(1)&gt;"));
    assert!(!html.contains("<svg onload=alert(1)>"));
    assert!(!html.contains("name=\"csrf_token\""));
    assert_eq!(pg(&url, &base_audit).await, "4");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM social_mail_admin_access_audit
                 WHERE admin_user_id={} AND action='read_group_conversation'
                   AND group_id={group_id} AND book_id IS NULL",
                users[0]
            )
        )
        .await,
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM social_mail_admin_access_audit
                 WHERE admin_user_id={} AND action='read_book_room'
                   AND book_id={book_id} AND group_id IS NULL",
                users[0]
            )
        )
        .await,
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM social_group_members
                 WHERE group_id={group_id} AND user_id={}",
                users[0]
            )
        )
        .await,
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM social_book_room_members
                 WHERE book_id={book_id} AND user_id={}",
                users[0]
            )
        )
        .await,
        "0"
    );
    for path in [
        "/admin/mail/groups?before=0",
        "/admin/mail/book-rooms?before=0",
        "/admin/mail/groups/0",
        "/admin/mail/book-rooms/0",
    ] {
        assert_eq!(
            call(&app, "GET", path, &cookies[0], "", &[]).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for path in [
        "/admin/mail/groups/9223372036854775807",
        "/admin/mail/book-rooms/9223372036854775807",
    ] {
        assert_eq!(
            call(&app, "GET", path, &cookies[0], "", &[]).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(pg(&url, &base_audit).await, "4");
    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=FALSE
             WHERE user_id={} AND role='administrator'",
            users[0]
        ),
    )
    .await;
    for path in [&group_path, &room_path] {
        assert_eq!(
            call(&app, "GET", path, &cookies[0], "", &[]).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(pg(&url, &base_audit).await, "4");
    // Never leave another active administrator for the A-04/A-05 PG tests.
}
