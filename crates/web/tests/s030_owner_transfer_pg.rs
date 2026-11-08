//! S030: only disposable PostgreSQL and real HTTP sessions. SQL fixtures run
//! via the established psql integration harness; web has no SQLx dependency.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    num::NonZeroU32,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::{
        book_step_up::{BookStepUpScope, BOOK_SHARES_SCOPE, BOOK_TRANSFER_SCOPE},
        PgAccounts,
    },
    services::security::{
        generate_session_token, hash_password, session_token_digest, verify_password_for_step_up,
    },
};
use tor_books::{
    domain::shares::ShareAssignment,
    repositories::{
        owner_transfer::{
            owner_transfer_payload_digest, OwnerTransferPrecondition, PgOwnerTransfer,
        },
        revenue_shares::{revenue_share_payload_digest, PgRevenueShares},
        translation_team_books::PgTeamBooks,
        PgCatalog,
    },
    services::authoring::AuthorError,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PASSWORD: &str = "S030SecureTransferPassword2026";

fn pg(url: &str, query: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let output = Command::new("psql")
        .env("PGOPTIONS", "-c lock_timeout=15000 -c statement_timeout=60000")
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
            query,
        ])
        .output()
        .expect("psql executable in isolated CI");
    assert!(
        output.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn database() -> String {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG URL");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    url
}

async fn user(accounts: &PgAccounts, email: &str, display: &str, hash: &str) -> (i64, String) {
    let token = generate_session_token();
    let id = accounts
        .register_with_session(
            email,
            display,
            hash,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (id, token)
}

fn ownership_revision(book: i64) -> i64 {
    pg(
        &database(),
        &format!(
            "SELECT COALESCE(MAX(id)+1,1) FROM book_owner_transfer_events WHERE book_id={book}"
        ),
    )
    .parse()
    .unwrap()
}

fn expected_owner(owner_id: i64, book: i64) -> OwnerTransferPrecondition {
    OwnerTransferPrecondition {
        owner_id,
        revision: ownership_revision(book),
    }
}

async fn transfer_token(
    accounts: &PgAccounts,
    actor: i64,
    book: i64,
    target: &str,
    session: &str,
    hash: &str,
) -> String {
    let verified = verify_password_for_step_up(PASSWORD, hash).unwrap();
    let payload = owner_transfer_payload_digest(actor, book, actor, target);
    accounts
        .issue_book_mutation_step_up(
            actor,
            &session_token_digest(session),
            &verified,
            book,
            ownership_revision(book),
            BookStepUpScope {
                name: BOOK_TRANSFER_SCOPE,
                payload_digest: &payload,
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s030_pg_one_owner_team_and_immutable_share_revisions() {
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let hash = hash_password(PASSWORD).unwrap();
    let unique = stamp();
    let recipient_name = format!("S030TeamRecipient{unique}");
    let (from, from_session) = user(
        &accounts,
        &format!("s030-team-owner-{unique}@example.invalid"),
        &format!("S030Owner{unique}"),
        &hash,
    )
    .await;
    let (to, to_session) = user(
        &accounts,
        &format!("s030-team-member-{unique}@example.invalid"),
        &recipient_name,
        &hash,
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:s030:{unique}:team','ru','S030 transfer','draft',{from},'basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let team: i64 = pg(
        &url,
        &format!(
            "INSERT INTO translation_teams(owner_user_id,name)
             VALUES({from},'S030 Team {unique}') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let invite = hex::encode(session_token_digest(&format!("s030-team-invite-{unique}")));
    pg(
        &url,
        &format!(
            "INSERT INTO translation_team_invites
             (token_digest,team_id,invitee_user_id,inviter_user_id,status,decided_at)
             VALUES(decode('{invite}','hex'),{team},{to},{from},'accepted',now())"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO translation_team_members(team_id,user_id,invite_digest)
             VALUES({team},{to},decode('{invite}','hex'))"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO translation_team_books(team_id,book_id,added_by_user_id)
             VALUES({team},{book},{from})"
        ),
    );
    let owner_service = PgOwnerTransfer::new(pool.clone());
    assert!(owner_service.settings(from, book).await.unwrap().is_some());
    assert!(owner_service.settings(to, book).await.unwrap().is_none());
    let token = transfer_token(&accounts, from, book, &recipient_name, &from_session, &hash).await;
    owner_service
        .transfer(
            from,
            book,
            expected_owner(from, book),
            &recipient_name,
            &session_token_digest(&from_session),
            &token,
        )
        .await
        .unwrap();

    assert_eq!(
        pg(
            &url,
            &format!("SELECT owner_user_id FROM books WHERE id={book}")
        ),
        to.to_string(),
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT COUNT(*) FROM book_members WHERE book_id={book}
             AND is_active AND role='owner'"
            )
        ),
        "1",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT COUNT(*) FROM book_owner_transfer_events
             WHERE book_id={book} AND previous_owner_user_id={from}
               AND new_owner_user_id={to} AND share_revision=1"
            )
        ),
        "1",
    );
    assert!(owner_service.settings(from, book).await.unwrap().is_none());
    assert!(owner_service.settings(to, book).await.unwrap().is_some());
    let team_view = PgTeamBooks::new(pool.clone()).view(to, team).await.unwrap();
    assert!(team_view.linked.iter().any(|row| row.id == book));
    assert_eq!(
        owner_service
            .transfer(
                from,
                book,
                expected_owner(from, book),
                &recipient_name,
                &session_token_digest(&from_session),
                &token,
            )
            .await,
        Err(AuthorError::Forbidden),
        "transfer confirmation cannot be replayed",
    );
    let assignments = [
        ShareAssignment {
            user_id: from,
            share_percent: 65,
        },
        ShareAssignment {
            user_id: to,
            share_percent: 35,
        },
    ];
    let reason = "new owner revision";
    let digest = revenue_share_payload_digest(to, book, 1, &assignments, reason).unwrap();
    let verified = verify_password_for_step_up(PASSWORD, &hash).unwrap();
    let share_token = accounts
        .issue_book_mutation_step_up(
            to,
            &session_token_digest(&to_session),
            &verified,
            book,
            1,
            BookStepUpScope {
                name: BOOK_SHARES_SCOPE,
                payload_digest: &digest,
            },
        )
        .await
        .unwrap();
    let shares = PgRevenueShares::new(pool.clone());
    let revision = shares
        .replace_schedule_verified(
            to,
            book,
            1,
            &assignments,
            reason,
            (&session_token_digest(&to_session), &share_token),
        )
        .await
        .unwrap();
    assert_eq!(revision, 2);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT share_percent FROM revenue_share_assignments
             WHERE book_id={book} AND revision=1 AND user_id={from}"
            )
        ),
        "100",
        "previous purchase conditions must remain immutable",
    );
    assert!(shares
        .replace_schedule_verified(
            to,
            book,
            2,
            &assignments,
            reason,
            (&session_token_digest(&to_session), &share_token),
        )
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s030_pg_rejects_completed_ordinary_deletion() {
    // Ordinary deletion preserves purchased chapters: is_deleted stays FALSE.
    // A transfer must not reopen this archived Book through the owner UI.
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let hash = hash_password(PASSWORD).unwrap();
    let unique = stamp();
    let target = format!("S030ArchivedRecipient{unique}");
    let (owner, session) = user(
        &accounts,
        &format!("s030-archived-owner-{unique}@example.invalid"),
        &format!("S030ArchivedOwner{unique}"),
        &hash,
    )
    .await;
    let (_recipient, _) = user(
        &accounts,
        &format!("s030-archived-target-{unique}@example.invalid"),
        &target,
        &hash,
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,
                               owner_user_id,access_level,lifecycle_status,ordinary_deleted_at)
             VALUES('ci:s030:{unique}:archived','ru','S030 archived','draft',
                    {owner},'basic','ordinary_deleted',clock_timestamp())
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!("SELECT is_deleted FROM books WHERE id={book}")
        ),
        "f",
        "ordinary deletion must not become a hard/legal deletion"
    );
    let token = transfer_token(&accounts, owner, book, &target, &session, &hash).await;
    assert_eq!(
        PgOwnerTransfer::new(catalog.pool())
            .transfer(
                owner,
                book,
                expected_owner(owner, book),
                &target,
                &session_token_digest(&session),
                &token,
            )
            .await,
        Err(AuthorError::Forbidden),
        "completed ordinary deletion must not be transferred"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT owner_user_id FROM books WHERE id={book}")
        ),
        owner.to_string()
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT COUNT(*) FROM book_owner_transfer_events WHERE book_id={book}")
        ),
        "0"
    );

    // Ordinary deletion leaves purchase records intact, but must close the
    // sensitive ownership page and block all future creator-share revisions.
    assert!(PgOwnerTransfer::new(catalog.pool())
        .settings(owner, book)
        .await
        .unwrap()
        .is_none());
    let assignments = [ShareAssignment {
        user_id: owner,
        share_percent: 100,
    }];
    let reason = "archived Book cannot change future shares";
    let digest = revenue_share_payload_digest(owner, book, 1, &assignments, reason).unwrap();
    let verified = verify_password_for_step_up(PASSWORD, &hash).unwrap();
    let share_token = accounts
        .issue_book_mutation_step_up(
            owner,
            &session_token_digest(&session),
            &verified,
            book,
            1,
            BookStepUpScope {
                name: BOOK_SHARES_SCOPE,
                payload_digest: &digest,
            },
        )
        .await
        .unwrap();
    let result = PgRevenueShares::new(catalog.pool())
        .replace_schedule_verified(
            owner,
            book,
            1,
            &assignments,
            reason,
            (&session_token_digest(&session), &share_token),
        )
        .await;
    assert!(matches!(
        result,
        Err(tor_books::domain::shares::RevenueShareError::Forbidden)
    ));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT current_revision FROM revenue_share_states WHERE book_id={book}")
        ),
        "1",
        "deleted Book must retain its immutable financial revision"
    );
}

// Test fixtures invoke synchronous psql after intentionally rejected SQLx
// transactions. Other workers must be able to finish the async rollback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s030_pg_rejects_low_tier_historic_layer_and_revoked_session() {
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let hash = hash_password(PASSWORD).unwrap();
    let unique = stamp();
    let target = format!("S030DeniedRecipient{unique}");
    let (from, from_session) = user(
        &accounts,
        &format!("s030-deny-owner-{unique}@example.invalid"),
        &format!("S030DeniedOwner{unique}"),
        &hash,
    )
    .await;
    let (to, _) = user(
        &accounts,
        &format!("s030-deny-target-{unique}@example.invalid"),
        &target,
        &hash,
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,
                               visibility,owner_user_id,access_level)
             VALUES('ci:s030:{unique}:deny','ru','S030 adult','draft',{from},'adult')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let owner_service = PgOwnerTransfer::new(catalog.pool());
    let digest = session_token_digest(&from_session);
    let age_token = transfer_token(&accounts, from, book, &target, &from_session, &hash).await;
    assert_eq!(
        owner_service
            .transfer(
                from,
                book,
                expected_owner(from, book),
                &target,
                &digest,
                &age_token,
            )
            .await,
        Err(AuthorError::Forbidden),
    );
    pg(
        &url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={to}"),
    );
    let layer: i64 = pg(
        &url,
        &format!(
            "INSERT INTO commerce_layers(owner_user_id,title)
             VALUES({from},'S030 history {unique}') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO commerce_layer_versions(layer_id,version_no)
             VALUES({layer},1) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!("UPDATE commerce_layers SET current_version_id={version} WHERE id={layer}"),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO commerce_layer_version_books(layer_version_id,book_id)
             VALUES({version},{book})"
        ),
    );
    let layer_token = transfer_token(&accounts, from, book, &target, &from_session, &hash).await;
    assert_eq!(
        owner_service
            .transfer(
                from,
                book,
                expected_owner(from, book),
                &target,
                &digest,
                &layer_token,
            )
            .await,
        Err(AuthorError::Conflict),
    );
    let revoked_token = transfer_token(&accounts, from, book, &target, &from_session, &hash).await;
    pg(
        &url,
        &format!(
            "UPDATE sessions SET revoked_at=now()
             WHERE token_hash=decode('{}','hex')",
            hex::encode(digest),
        ),
    );
    assert_eq!(
        owner_service
            .transfer(
                from,
                book,
                expected_owner(from, book),
                &target,
                &digest,
                &revoked_token,
            )
            .await,
        Err(AuthorError::Forbidden),
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT owner_user_id FROM books WHERE id={book}")
        ),
        from.to_string(),
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT COUNT(*) FROM book_owner_transfer_events WHERE book_id={book}")
        ),
        "0",
    );
}

fn request(method: &str, path: &str, cookie: &str, origin: &str, form: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap()
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    form: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(request(method, path, cookie, origin, form))
        .await
        .unwrap()
}

fn csrf_cookie(reply: &axum::response::Response) -> String {
    reply
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("tor-dev-csrf="))
        .expect("CSRF cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn text(reply: axum::response::Response) -> String {
    let bytes = to_bytes(reply.into_body(), 500_000).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn csrf_from(html: &str) -> &str {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s030_http_owner_forms_origin_csrf_step_up_transfer_and_share_revision() {
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let unique = stamp();
    let hash = hash_password(PASSWORD).unwrap();
    let recipient_name = format!("S030HttpRecipient{unique}");
    let (from, from_token) = user(
        &accounts,
        &format!("s030-http-owner-{unique}@example.invalid"),
        &format!("S030HttpOwner{unique}"),
        &hash,
    )
    .await;
    let (to, to_token) = user(
        &accounts,
        &format!("s030-http-recipient-{unique}@example.invalid"),
        &recipient_name,
        &hash,
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:s030:{unique}:http','ru','S030 HTTP','draft',{from},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_cookie = format!("tor-dev-session={from_token}");
    let foreign_cookie = format!("tor-dev-session={to_token}");
    let path = format!("/me/books/{book}/settings/ownership");
    let initial = send(&app, "GET", &path, &owner_cookie, ORIGIN, "").await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(initial.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_pair = csrf_cookie(&initial);
    let initial_html = text(initial).await;
    let csrf = csrf_from(&initial_html);
    assert!(initial_html.contains("share_"));
    assert!(initial_html.contains("expected_ownership_revision\" value=\"1\""));
    let owner_auth = format!("{owner_cookie}; {csrf_pair}");
    assert_eq!(
        send(&app, "GET", &path, &foreign_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND,
    );
    let transfer_path = format!("{path}/transfer");
    let form = format!(
        "csrf_token={csrf}&expected_owner_id={from}&expected_ownership_revision=1&target_display_name={recipient_name}&account_password={PASSWORD}"
    );
    assert_eq!(
        send(&app, "POST", &transfer_path, &owner_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "missing CSRF cookie",
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &transfer_path,
            &owner_auth,
            "https://evil.example",
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "wrong Origin",
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &transfer_path,
            &owner_auth,
            ORIGIN,
            &form.replace(PASSWORD, "incorrect-password")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED,
        "incorrect account password must reject transfer",
    );
    let transferred = send(&app, "POST", &transfer_path, &owner_auth, ORIGIN, &form).await;
    assert_eq!(transferred.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        pg(
            &url,
            &format!("SELECT owner_user_id FROM books WHERE id={book}")
        ),
        to.to_string(),
    );
    assert_eq!(
        send(&app, "GET", &path, &owner_auth, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND,
    );
    let recipient_page = send(&app, "GET", &path, &foreign_cookie, ORIGIN, "").await;
    assert_eq!(recipient_page.status(), StatusCode::OK);
    let to_csrf_cookie = csrf_cookie(&recipient_page);
    let recipient_html = text(recipient_page).await;
    let to_csrf = csrf_from(&recipient_html);
    assert!(recipient_html.contains("expected_revision\" value=\"1\""));
    let to_auth = format!("{foreign_cookie}; {to_csrf_cookie}");
    let shares_path = format!("{path}/shares");
    let finance_form = format!(
        "csrf_token={to_csrf}&expected_revision=1&share_{from}=40&share_{to}=60&reason=After+transfer&account_password={PASSWORD}"
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &shares_path,
            &owner_auth,
            ORIGIN,
            &finance_form
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "former owner cannot change creator economics",
    );
    assert_eq!(
        send(&app, "POST", &shares_path, &to_auth, ORIGIN, &finance_form)
            .await
            .status(),
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT current_revision FROM revenue_share_states WHERE book_id={book}")
        ),
        "2",
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT share_percent FROM revenue_share_assignments
             WHERE book_id={book} AND revision=1 AND user_id={from}"
            )
        ),
        "100",
        "historical shares must not change",
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s030_pg_parallel_user_transfer_consumes_proof_exactly_once() {
    let url = database();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let hash = hash_password(PASSWORD).unwrap();
    let unique = stamp();
    let recipient_name = format!("S030RaceRecipient{unique}");
    let (owner, session) = user(
        &accounts,
        &format!("s030-race-owner-{unique}@example.invalid"),
        &format!("S030RaceOwner{unique}"),
        &hash,
    )
    .await;
    let (recipient, _) = user(
        &accounts,
        &format!("s030-race-recipient-{unique}@example.invalid"),
        &recipient_name,
        &hash,
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:s030:{unique}:race','ru','S030 concurrent transfer','draft',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let token = transfer_token(&accounts, owner, book, &recipient_name, &session, &hash).await;
    let digest = session_token_digest(&session);
    let service = PgOwnerTransfer::new(pool);
    let (first, second) = tokio::join!(
        service.transfer(
            owner,
            book,
            expected_owner(owner, book),
            &recipient_name,
            &digest,
            &token
        ),
        service.transfer(
            owner,
            book,
            expected_owner(owner, book),
            &recipient_name,
            &digest,
            &token
        ),
    );
    assert_eq!(
        [first, second]
            .iter()
            .filter(|result| result.is_ok())
            .count(),
        1,
        "only one concurrent HTTP-equivalent transfer may commit",
    );
    assert_eq!(
        [first, second]
            .iter()
            .filter(|result| **result == Err(AuthorError::Forbidden))
            .count(),
        1,
        "the same password proof cannot authorize a second transfer",
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT owner_user_id FROM books WHERE id={book}")
        ),
        recipient.to_string(),
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT COUNT(*) FROM book_members
                 WHERE book_id={book} AND role='owner' AND is_active"
            ),
        ),
        "1",
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT COUNT(*) FROM book_owner_transfer_events WHERE book_id={book}"),
        ),
        "1",
        "a racing duplicate must not append another audit event",
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT current_revision FROM revenue_share_states WHERE book_id={book}"),
        ),
        "1",
        "ownership transfer must not implicitly change the revenue split",
    );
}

#[path = "s030_owner_transfer_pg/aba.rs"]
mod aba;
