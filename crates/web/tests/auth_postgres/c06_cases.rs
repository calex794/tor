//! C-06/S006: HTTP evidence for owned, pinned paid reading progress.
//! No paid chapter body may ever be returned by these endpoints.
use super::*;

const PURCHASE: &str = "95840";
const CHAPTER: &str = "95820";
const V1: &str = "95831";
const GET_V1: &str =
    "/api/v1/me/progress/purchased?purchase_id=95840&chapter_id=95820&version_id=95831";
const SAVE: &str = "/api/v1/me/progress/purchased";
const SECRET: &str = "C06-PRIVATE-PAID-MARKER";

fn test_sql(sql: &str) {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable test database");
    assert!(url.ends_with("/tor_ci"), "never write to production");
    let output = std::process::Command::new("psql")
        .args(["-X", "-q", "-v", "ON_ERROR_STOP=1", "-d", &url, "-c", sql])
        .output()
        .expect("PostgreSQL client in integration CI");
    assert!(
        output.status.success(),
        "isolated test fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn purchase_request(offset: i32, block: i32, revision: i64) -> String {
    format!(
        r#"{{"purchase_id":"{PURCHASE}","chapter_id":"{CHAPTER}","version_id":"{V1}","block_id":{block},"char_offset":{offset},"expected_revision":{revision}}}"#
    )
}

async fn owned_get(app: &Router, cookies: &str, uri: &str, status: StatusCode) -> String {
    let reply = send(app, json_request("GET", uri, "", cookies, ORIGIN, "")).await;
    assert_eq!(reply.status(), status, "{uri}");
    assert_eq!(reply.headers()[header::CACHE_CONTROL], "no-store");
    let body = body_of(reply).await;
    assert!(
        !body.contains(SECRET),
        "paid plaintext leaked into progress API"
    );
    if status != StatusCode::OK {
        assert!(body.is_empty(), "denial must not reveal paid metadata");
    }
    body
}

pub(super) async fn purchased_version_http_journey(
    app: &Router,
    first_session: &str,
    second_session: &str,
) {
    // The test user was registered through the public HTTP form beforehand.
    // Settlement facts are inserted only into the disposable PG database;
    // the public product has no enabled payment or paid-delivery route.
    eprintln!("C06 checkpoint: start fixture");
    test_sql(
        r#"BEGIN;
           INSERT INTO users(id,email_normalized,display_name,password_hash)
             OVERRIDING SYSTEM VALUE VALUES
             (95801,'c06-other@example.invalid','Other','$argon2id$fixture');
           INSERT INTO books(id,source_identity,target_language,title,visibility)
             VALUES(95810,'ci:c06:pinned','ru','Private test book','public');
           INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES(95820,95810,1,'Paid chapter','public','paid',100);
           INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
             VALUES(95831,95820,1,E'А😀Б\n\nC06-PRIVATE-PAID-MARKER'),
                   (95832,95820,2,'C06-PRIVATE-V2-MARKER');
           INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES(95820,95831);
           INSERT INTO chapter_purchase_facts
             (id,user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
             SELECT 95840,id,95820,95831,'ci-c06-own-purchase',100,'RUB'
               FROM users WHERE email_normalized='tor_ci_account@example.com';
           INSERT INTO chapter_purchase_facts
             (id,user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
             VALUES(95841,95801,95820,95831,'ci-c06-other-purchase',100,'RUB');
           COMMIT;"#,
    );

    assert_eq!(
        send(
            app,
            request("GET", &format!("/reader/{CHAPTER}"), "", "", ORIGIN)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "paid body must not be served by public reader"
    );
    let anonymous = owned_get(app, "", GET_V1, StatusCode::UNAUTHORIZED).await;
    assert!(anonymous.is_empty());
    let other_purchase =
        "/api/v1/me/progress/purchased?purchase_id=95841&chapter_id=95820&version_id=95831";
    owned_get(app, first_session, other_purchase, StatusCode::NOT_FOUND).await;
    let first = owned_get(app, first_session, GET_V1, StatusCode::OK).await;
    assert!(first.contains(r#""revision":0"#));
    assert!(first.contains(r#""block_id":null"#));

    let page_a = send(app, request("GET", "/read/101", "", first_session, ORIGIN)).await;
    assert_eq!(page_a.status(), StatusCode::OK);
    let cookie_a = cookie(&page_a, "tor-dev-csrf=");
    let csrf_a = csrf_token(&body_of(page_a).await).to_owned();
    let cookies_a = format!("{first_session}; {cookie_a}");

    let page_b = send(app, request("GET", "/read/101", "", second_session, ORIGIN)).await;
    assert_eq!(page_b.status(), StatusCode::OK);
    let cookie_b = cookie(&page_b, "tor-dev-csrf=");
    let csrf_b = csrf_token(&body_of(page_b).await).to_owned();
    let cookies_b = format!("{second_session}; {cookie_b}");

    eprintln!("C06 checkpoint: purchase owner checks done");
    let initial = purchase_request(3, 1, 0); // A + surrogate pair: 3 UTF-16 units.
    for (cookies, origin, csrf) in [
        (first_session, ORIGIN, csrf_a.as_str()),
        (cookies_a.as_str(), "https://evil.example", csrf_a.as_str()),
        (cookies_a.as_str(), ORIGIN, ""),
    ] {
        let denied = send(
            app,
            json_request("PUT", SAVE, &initial, cookies, origin, csrf),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(denied.headers()[header::CACHE_CONTROL], "no-store");
        assert!(body_of(denied).await.is_empty());
    }
    let invalid = purchase_request(5, 1, 0); // First block has four UTF-16 units.
    let invalid_response = send(
        app,
        json_request("PUT", SAVE, &invalid, &cookies_a, ORIGIN, &csrf_a),
    )
    .await;
    assert_eq!(invalid_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body_of(invalid_response).await.is_empty());

    eprintln!("C06 checkpoint: first paid cursor save");
    let saved = send(
        app,
        json_request("PUT", SAVE, &initial, &cookies_a, ORIGIN, &csrf_a),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(saved.headers()[header::CACHE_CONTROL], "no-store");
    let receipt = body_of(saved).await;
    assert!(receipt.contains(r#""revision":1"#));
    assert!(!receipt.contains(SECRET));

    // A second independently logged-in device sees the same pinned V1 cursor.
    let after_login = owned_get(app, second_session, GET_V1, StatusCode::OK).await;
    assert!(after_login.contains(r#""revision":1"#));
    assert!(after_login.contains(r#""char_offset":3"#));

    let left = purchase_request(1, 2, 1);
    let right = purchase_request(2, 2, 1);
    eprintln!("C06 checkpoint: competing paid cursor saves");
    let (one, two) = tokio::join!(
        send(
            app,
            json_request("PUT", SAVE, &left, &cookies_a, ORIGIN, &csrf_a)
        ),
        send(
            app,
            json_request("PUT", SAVE, &right, &cookies_b, ORIGIN, &csrf_b)
        ),
    );
    let statuses = [one.status(), two.status()];
    assert_eq!(statuses.iter().filter(|&&s| s == StatusCode::OK).count(), 1);
    assert_eq!(
        statuses
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    let current = owned_get(app, second_session, GET_V1, StatusCode::OK).await;
    assert!(current.contains(r#""revision":2"#));
    assert!(current.contains(r#""block_id":2"#));

    // A V2 publication must not erase the V1 purchase or move its cursor.
    eprintln!("C06 checkpoint: pinned V1 after V2");
    test_sql("UPDATE chapter_publications SET version_id=95832 WHERE chapter_id=95820");
    let preserved = owned_get(app, first_session, GET_V1, StatusCode::OK).await;
    assert!(preserved.contains(r#""version_id":"95831""#));
    assert!(preserved.contains(r#""revision":2"#));
    let v2 = "/api/v1/me/progress/purchased?purchase_id=95840&chapter_id=95820&version_id=95832";
    owned_get(app, first_session, v2, StatusCode::NOT_FOUND).await;
    assert_eq!(
        send(
            app,
            request(
                "GET",
                &format!("/read/{CHAPTER}"),
                "",
                first_session,
                ORIGIN
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "pinned purchase does not make the public reader reveal paid bytes"
    );

    // F-05: finishing the still-owned pinned V1 after V2 release must
    // create a C event and immediately make this book privately rankable.
    // A mid-chapter progress PUT above did not create any C completion.
    let final_offset = "C06-PRIVATE-PAID-MARKER".encode_utf16().count() as i32;
    let final_save = purchase_request(final_offset, 2, 2);
    eprintln!("C06 checkpoint: paid V1 completion");
    let completed = send(
        app,
        json_request("PUT", SAVE, &final_save, &cookies_a, ORIGIN, &csrf_a),
    )
    .await;
    assert_eq!(completed.status(), StatusCode::OK);
    let completed_body = body_of(completed).await;
    assert!(completed_body.contains(r#""revision":3"#));
    assert!(!completed_body.contains(SECRET));
    eprintln!("C06 checkpoint: read paid ranking");
    let ranking = send(
        app,
        request("GET", "/api/v1/me/rankings", "", first_session, ORIGIN),
    )
    .await;
    assert_eq!(ranking.status(), StatusCode::OK);
    let ranked_json = body_of(ranking).await;
    assert!(ranked_json.contains("Private test book"));
    assert!(!ranked_json.contains(SECRET));
    // A second device trying to submit the same end is stale, not a new rank.
    let repeat = send(
        app,
        json_request("PUT", SAVE, &final_save, &cookies_b, ORIGIN, &csrf_b),
    )
    .await;
    assert_eq!(repeat.status(), StatusCode::CONFLICT);

    // A refund revokes both reads and subsequent saves immediately.
    eprintln!("C06 checkpoint: revoke purchase");
    test_sql(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES (95840,'ci-c06-refund','refund')",
    );
    owned_get(app, first_session, GET_V1, StatusCode::NOT_FOUND).await;
    let attempt = purchase_request(3, 2, 2);
    let revoked = send(
        app,
        json_request("PUT", SAVE, &attempt, &cookies_a, ORIGIN, &csrf_a),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::NOT_FOUND);
    assert_eq!(revoked.headers()[header::CACHE_CONTROL], "no-store");
    assert!(body_of(revoked).await.is_empty());
}
