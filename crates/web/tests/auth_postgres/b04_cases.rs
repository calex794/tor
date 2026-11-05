//! Isolated B-04 author browser scenario, split to keep each test file bounded.
use super::*;

pub(super) async fn draft_edit_case(
    app: &Router,
    book_id: i64,
    book_path: &str,
    session: &str,
    author_cookies: &str,
    author_book_html: &str,
) -> String {
    // B-04: browser edit reads the stored working text and expected revision.
    let edit_prefix = format!("href=\"/me/books/{book_id}/chapters/");
    let chapter_id = author_book_html
        .split(&edit_prefix)
        .nth(1)
        .expect("draft edit link")
        .split("/edit\"")
        .next()
        .expect("chapter ID");
    let edit_path = format!("/me/books/{book_id}/chapters/{chapter_id}/edit");
    assert_eq!(
        send(app, request("GET", &edit_path, "", "", ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let editor = send(app, request("GET", &edit_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(editor.status(), StatusCode::OK);
    assert_eq!(editor.headers()[header::CACHE_CONTROL], "no-store");
    let edit_html = body_of(editor).await;
    assert!(edit_html.contains("name=\"expected_revision\" value=\"1\""));
    assert!(edit_html.contains("Real chapter"));
    let edit_key = edit_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .expect("edit key")
        .split('"')
        .next()
        .unwrap();
    let edit_csrf = csrf_token(&edit_html);
    let edit_payload = format!(
        "number=1&title=%D0%93%D0%BB%D0%B0%D0%B2%D0%B0&body=%23%20Updated%20chapter&expected_revision=1&command_key={edit_key}&csrf_token={edit_csrf}"
    );
    assert_eq!(
        send(
            app,
            request("POST", &edit_path, &edit_payload, session, ORIGIN)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            app,
            request(
                "POST",
                &edit_path,
                &edit_payload,
                author_cookies,
                "https://evil.example"
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let updated = send(
        app,
        request("POST", &edit_path, &edit_payload, author_cookies, ORIGIN),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        updated.headers()[header::LOCATION].to_str().unwrap(),
        book_path
    );
    assert_eq!(
        send(
            app,
            request("POST", &edit_path, &edit_payload, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "repeat of a successful command must be harmless"
    );
    let stale_payload = edit_payload.replace(edit_key, &"s".repeat(43));
    assert_eq!(
        send(
            app,
            request("POST", &edit_path, &stale_payload, author_cookies, ORIGIN)
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "stale tab must not overwrite revision two"
    );
    let editor_again = send(app, request("GET", &edit_path, "", author_cookies, ORIGIN)).await;
    assert_eq!(editor_again.status(), StatusCode::OK);
    let updated_html = body_of(editor_again).await;
    assert!(updated_html.contains("name=\"expected_revision\" value=\"2\""));
    assert!(updated_html.contains("Updated chapter"));
    edit_path
}

// Test-only site approval: author release cannot bypass moderation.
// HTTP site-role, CSRF, CAS and audit are verified in moderation_books_pg.
pub(super) async fn approve_pending_book_ci(app: &Router, book_id: i64) {
    let path = format!("/books/{book_id}");
    assert_eq!(
        send(app, request("GET", &path, "", "", ORIGIN))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "author release must remain private until site approval"
    );
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL test URL");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    // Independent, active site moderator: exercise the real registration
    // decision repository rather than bypassing the SQL approval trigger.
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let token = tor_accounts::services::security::generate_session_token();
    let digest = tor_accounts::services::security::session_token_digest(&token);
    let reviewer_password =
        tor_accounts::services::security::hash_password("ModregB04Reviewer2026!").unwrap();
    let reviewer = accounts
        .register_with_session(
            &format!("modreg-b04-{book_id}@tor-ci.example"),
            "CI site registration reviewer",
            &reviewer_password,
            &digest,
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let role_sql = format!(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({reviewer},'moderator',TRUE)"
    );
    let role = std::process::Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            &url,
            "--command",
            &role_sql,
        ])
        .output()
        .expect("psql installed on disposable CI");
    assert!(
        role.status.success(),
        "{}",
        String::from_utf8_lossy(&role.stderr)
    );
    let sql = format!(
        "SELECT visibility_status_revision FROM books
         WHERE id={book_id} AND visibility='public'
         AND visibility_status='pending_moderation'"
    );
    let output = std::process::Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            &url,
            "--command",
            &sql,
        ])
        .output()
        .expect("psql installed on disposable CI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected_revision: i64 = String::from_utf8(output.stdout)
        .expect("psql UTF-8")
        .trim()
        .parse()
        .expect("one pending Book revision");
    accounts
        .moderate_book_registration(&digest, book_id, expected_revision, "approve", "")
        .await
        .expect("independent site moderator approves registration");
    assert_eq!(
        send(app, request("GET", &path, "", "", ORIGIN))
            .await
            .status(),
        StatusCode::OK,
        "approved registration keeps the author's released Book visible"
    );
}
