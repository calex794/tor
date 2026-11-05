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
    // The test fixture records a complete, independent site review instead
    // of setting the active flag directly. The SQL trigger must stay enabled.
    let password_hash = tor_accounts::services::security::hash_password(
        "CI-Only-Independent-Moderator-2026!",
    )
    .expect("Argon2id for disposable reviewer");
    let sql = format!(
        "DO $ci$
         DECLARE previous_rev bigint; owner bigint; reviewer bigint; decision bigint;
         BEGIN
           SELECT visibility_status_revision,owner_user_id INTO previous_rev,owner
             FROM books WHERE id={book_id} AND visibility='public'
               AND visibility_status='pending_moderation'
               AND registration_approval_decision_id IS NULL FOR UPDATE;
           IF previous_rev IS NULL THEN
             RAISE EXCEPTION 'Book is not pending registration';
           END IF;
           INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('ci-b04-reviewer-{book_id}@example.invalid',
                    'Independent CI reviewer','{password_hash}')
             RETURNING id INTO reviewer;
           IF reviewer=owner THEN
             RAISE EXCEPTION 'Book owner cannot approve registration';
           END IF;
           INSERT INTO account_roles(user_id,role,is_active)
             VALUES(reviewer,'moderator',TRUE);
           INSERT INTO book_registration_decisions
             (book_id,reviewer_user_id,outcome,expected_visibility_revision,
              resulting_visibility_revision)
             VALUES({book_id},reviewer,'approved',previous_rev,previous_rev+1)
             RETURNING id INTO decision;
           UPDATE books SET visibility_status='active',
             visibility_status_revision=previous_rev+1,
             registration_approval_decision_id=decision
             WHERE id={book_id} AND visibility_status_revision=previous_rev;
           IF NOT FOUND THEN
             RAISE EXCEPTION 'registration revision changed';
           END IF;
           INSERT INTO moderation_book_visibility_events
             (actor_user_id,book_id,before_status,after_status,before_revision,after_revision)
             VALUES(reviewer,{book_id},'pending_moderation','active',
                    previous_rev,previous_rev+1);
         END
         $ci$;
         SELECT id FROM books WHERE id={book_id}
           AND registration_approval_decision_id IS NOT NULL
           AND visibility_status='active';"
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
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("psql output")
            .trim(),
        book_id.to_string(),
        "exactly one pending book must be approved"
    );
}
