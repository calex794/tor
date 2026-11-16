//! A/S002 R1: a deferred PostgreSQL COMMIT error must not mint a session
//! or leave a registered account after the account, session and login event
//! have all been staged in the same transaction.
use super::*;
use std::{
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

// tor-web is intentionally forbidden to depend on SQLx. The real SQL
// injection and witness operate through disposable CI PostgreSQL's psql.
fn pg(url: &str, statement: &str) -> String {
    assert!(
        url == "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci"
            || url == "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci",
        "only disposable CI databases may receive DDL"
    );
    let output = Command::new("psql")
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
        .expect("psql installed on isolated PG runner");
    assert!(
        output.status.success(),
        "CI PostgreSQL setup/witness error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("psql UTF-8")
        .trim()
        .to_owned()
}

// Account-specific witnesses avoid racing unrelated auth tests running
// concurrently in this same binary on the isolated CI database.
fn signup_facts(url: &str, email: &str) -> (i64, i64) {
    assert!(email.starts_with("a02-deferred-") && email.ends_with("@example.invalid"));
    let query = format!(
        "SELECT (SELECT count(*) FROM sessions s JOIN users u ON u.id=s.user_id WHERE u.email_normalized='{email}'), (SELECT count(*) FROM account_login_events e JOIN users u ON u.id=e.user_id WHERE u.email_normalized='{email}')"
    );
    let counts = pg(url, &query);
    let (sessions, events) = counts.split_once('|').expect("two CI counters");
    (sessions.parse().unwrap(), events.parse().unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL 18"]
async fn a_02_deferred_registration_commit_error_rolls_back_account_and_session() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable app PostgreSQL");
    assert_eq!(
        url, "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
        "fault injection must never run against a production database"
    );
    let migrator_url = std::env::var("MIGRATOR_URL").expect("disposable PG migrator");
    assert_eq!(
        migrator_url,
        "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci"
    );

    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("a02-deferred-{nonce}@example.invalid");
    let form_email = email.replace('@', "%40");
    let page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let html = body_of(page).await;
    let csrf = csrf_token(&html);
    let form = format!(
        "email={form_email}&display_name=CommitReader&password=LongSecurePassword2026&csrf_token={csrf}"
    );

    // This isolated trigger is evaluated only at COMMIT. It observes a login
    // event emitted by the same transaction as the new user and session, so
    // the HTTP operation has passed all three staging writes before failing.
    // Match this test-specific email prefix to avoid altering other tests.
    pg(
        &migrator_url,
        "CREATE FUNCTION ci_a02_reject_signup_commit() RETURNS trigger AS
         'BEGIN
            IF EXISTS (
              SELECT 1 FROM users
              WHERE id=NEW.user_id
                AND email_normalized LIKE ''a02-deferred-%@example.invalid''
            ) THEN
              RAISE EXCEPTION ''A02 injected registration COMMIT rejection''
                USING ERRCODE = ''40001'';
            END IF;
            RETURN NULL;
          END'
         LANGUAGE plpgsql",
    );
    pg(
        &migrator_url,
        "CREATE CONSTRAINT TRIGGER ci_a02_deferred_signup_commit
         AFTER INSERT ON account_login_events
         DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION ci_a02_reject_signup_commit()",
    );
    let before = signup_facts(&url, &email);
    assert_eq!(before, (0, 0));
    let rejected = tokio::time::timeout(
        Duration::from_secs(30),
        send(
            &app,
            request("POST", "/register", &form, &csrf_cookie, ORIGIN),
        ),
    )
    .await
    .expect("COMMIT injection must not hang the registration request");
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(rejected.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        rejected
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .all(|value| !value.to_str().unwrap_or("").starts_with("tor-dev-session=")),
        "an aborted registration must never expose a live session cookie"
    );
    assert!(
        accounts.find_user(&email).await.unwrap().is_none(),
        "user creation must be rolled back with the failed COMMIT"
    );
    assert_eq!(
        signup_facts(&url, &email),
        before,
        "COMMIT rollback must remove this signup's session and audit event"
    );

    pg(
        &migrator_url,
        "DROP TRIGGER ci_a02_deferred_signup_commit ON account_login_events",
    );
    pg(&migrator_url, "DROP FUNCTION ci_a02_reject_signup_commit()");

    // The exact same form succeeds once after database recovery. A duplicate
    // replay cannot create a second user/session or displace the first token.
    let created = send(
        &app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let session = cookie(&created, "tor-dev-session=");
    let token = session.split_once('=').unwrap().1;
    let digest = tor_accounts::services::security::session_token_digest(token);
    let user = accounts
        .find_user(&email)
        .await
        .unwrap()
        .expect("recovered registration persists one user");
    assert_eq!(
        accounts.active_session_user(&digest).await.unwrap(),
        Some(user.id)
    );
    let sessions = accounts
        .list_active_sessions(user.id, &digest)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].current);
    let duplicate = send(
        &app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(
        accounts
            .list_active_sessions(user.id, &digest)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        send(&app, request("GET", "/me", "", &session, ORIGIN))
            .await
            .status(),
        StatusCode::OK
    );
    pool.close().await;
}
