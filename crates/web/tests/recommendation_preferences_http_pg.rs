//! Explicit book choices are account settings, independent of optional analytics.
//! Real A/F HTTP and PostgreSQL; no fake preference store or disabled ACL.
#![forbid(unsafe_code)]
#[path = "recommendation_preferences_http_pg/fixtures.rs"]
mod fixtures;

use axum::{http::StatusCode, response::Response};
use fixtures::*;
#[path = "recommendation_preferences_http_pg/contextual.rs"]
mod contextual;
#[path = "support/collection_retirement.rs"]
mod retirement;
use retirement::{assert_current_schema, optional_snapshot};
use serde_json::{json, Value};
use tor_accounts::services::security::generate_session_token;

async fn receipt(response: Response, revision: i64, hidden: bool) {
    private(&response, StatusCode::OK);
    let body = json(response).await;
    assert_eq!(body["revision"], revision);
    assert_eq!(body["hidden"], hidden);
}

async fn conflict(response: Response, code: &str) {
    private(&response, StatusCode::CONFLICT);
    assert_eq!(json(response).await["error"]["code"], code);
}

fn consent_form(actor: &Actor, enabled: bool, revision: i64, key: &str) -> String {
    format!(
        "enabled={}&expected_revision={revision}&command_key={key}&csrf_token={}",
        if enabled { "1" } else { "" },
        actor.csrf
    )
}

async fn feed(h: &Harness, actor: &Actor) -> Value {
    let response = h.get(actor, "/api/v1/recommendations?limit=20").await;
    private(&response, StatusCode::OK);
    json(response).await
}

fn contains_book(feed: &Value, book: i64) -> bool {
    let book_id = book.to_string();
    feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["book_id"].as_str() == Some(book_id.as_str()))
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn explicit_choice_without_subject_has_real_replay_cas_and_actor_isolation() {
    let h = Harness::new(true).await;
    let actor = &h.actor;
    let auth = (ORIGIN, actor.csrf.as_str());
    h.assert_no_analytics_subject(actor);
    let offered = feed(&h, actor).await;
    let book_id = h.book.to_string();
    let card = offered["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["book_id"].as_str() == Some(book_id.as_str()))
        .unwrap();
    assert_eq!(card["feedback_revision"], 0);
    assert!(card["ticket"].is_null());
    h.assert_no_analytics_subject(actor);

    let hide = choice(h.book, true, 0);
    let hide_key = generate_session_token();
    receipt(h.feedback(actor, &hide, &hide_key, auth).await, 1, true).await;
    let state = h.preference(actor);
    let command = h.command(actor, &hide_key);
    assert_eq!(state["hidden"], true);
    assert_eq!(state["revision"], 1);
    h.assert_no_analytics_subject(actor);

    let settings = h.get(actor, "/me/recommendations").await;
    private(&settings, StatusCode::OK);
    let html = text(settings).await;
    assert!(html.contains("он не включает учёт посещений и чтения"));
    assert!(!html.contains("action=\"/me/recommendations/consent\""));
    assert!(!html.contains("data-flow-consent"));
    assert!(html.contains("&lt;script&gt;fixture&lt;/script&gt;"));
    assert!(!html.contains("<script>fixture</script>"));
    assert_eq!(h.preference(actor), state, "GET must not renew the TTL");
    assert!(!contains_book(&feed(&h, actor).await, h.book));
    assert!(contains_book(&feed(&h, &h.other).await, h.book));

    receipt(h.feedback(actor, &hide, &hide_key, auth).await, 1, true).await;
    assert_eq!(h.preference(actor), state);
    assert_eq!(h.command(actor, &hide_key), command);

    conflict(
        h.feedback(actor, &choice(h.book, false, 1), &hide_key, auth)
            .await,
        "IDEMPOTENCY_CONFLICT",
    )
    .await;
    conflict(
        h.feedback(actor, &hide, &generate_session_token(), auth)
            .await,
        "VERSION_CONFLICT",
    )
    .await;
    assert_eq!(h.preference(actor), state);

    // The same key belongs to the other actor's namespace, never to this actor.
    receipt(
        h.feedback(&h.other, &hide, &hide_key, (ORIGIN, &h.other.csrf))
            .await,
        1,
        true,
    )
    .await;
    assert_eq!(h.preference(actor), state);
    assert_eq!(h.preference(&h.other)["hidden"], true);
    h.assert_no_analytics_subject(&h.other);
    let forged = json!({
        "book_id": h.book.to_string(),
        "choice": "clear",
        "expected_revision": 1,
        "user_id": h.other.id,
    });
    private(
        &h.feedback(actor, &forged, &generate_session_token(), auth)
            .await,
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    assert_eq!(h.preference(&h.other)["hidden"], true);

    receipt(
        h.feedback(
            actor,
            &choice(h.book, false, 1),
            &generate_session_token(),
            auth,
        )
        .await,
        2,
        false,
    )
    .await;
    let cleared = h.preference(actor);
    receipt(h.feedback(actor, &hide, &hide_key, auth).await, 1, true).await;
    assert_eq!(
        h.preference(actor),
        cleared,
        "replaying a committed hide must not reverse the later clear"
    );
    assert_eq!(json(h.get(actor, FEEDBACK).await).await, json!([]));
    assert_eq!(h.preference(&h.other)["hidden"], true);
    assert!(contains_book(&feed(&h, actor).await, h.book));
    h.assert_no_analytics_subject(actor);
    h.cleanup();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn explicit_choice_survives_database_retirement_and_old_client_commands() {
    let h = Harness::new(true).await;
    let actor = &h.actor;
    let auth = (ORIGIN, actor.csrf.as_str());
    assert_current_schema(&h.url);
    h.assert_no_analytics_subject(actor);
    let scope = optional_snapshot(&h.url, actor.id);
    let hide_key = generate_session_token();
    let hide = choice(h.book, true, 0);
    receipt(h.feedback(actor, &hide, &hide_key, auth).await, 1, true).await;
    let preference = h.preference(actor);
    let command = h.command(actor, &hide_key);
    let head_sql = format!(
        "SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id={}",
        actor.id
    );
    let epoch = h.sql(&head_sql);
    assert_eq!(optional_snapshot(&h.url, actor.id), scope);
    for endpoint in [
        "/me/recommendations/consent",
        "/me/recommendations/flow-consent",
    ] {
        for enabled in [false, true] {
            // Even a previously used preference key is not consumed by a
            // tombstone; no consent revision, preference or receipt changes.
            let response = h
                .form(actor, endpoint, &consent_form(actor, enabled, 7, &hide_key))
                .await;
            private(&response, StatusCode::GONE);
            assert_eq!(json(response).await["error"]["code"], "COLLECTION_RETIRED");
            assert_eq!(optional_snapshot(&h.url, actor.id), scope);
            assert_eq!(h.preference(actor), preference);
            assert_eq!(h.command(actor, &hide_key), command);
            assert_eq!(h.sql(&head_sql), epoch);
        }
    }
    let hidden = h.get(actor, FEEDBACK).await;
    private(&hidden, StatusCode::OK);
    let hidden = json(hidden).await;
    assert_eq!(hidden.as_array().unwrap().len(), 1);
    assert_eq!(hidden[0]["book_id"], h.book.to_string());
    assert_eq!(hidden[0]["revision"], 1);
    assert!(!contains_book(&feed(&h, actor).await, h.book));
    assert!(contains_book(&feed(&h, &h.other).await, h.book));
    h.assert_no_analytics_subject(&h.other);
    let settings = text(h.get(actor, "/me/recommendations").await).await;
    assert!(!settings.contains("data-flow-consent"));
    assert!(!settings.contains("/me/recommendations/consent"));
    assert!(!settings.contains("data-rec-ticket"));
    receipt(h.feedback(actor, &hide, &hide_key, auth).await, 1, true).await;
    assert_eq!(h.preference(actor), preference);
    assert_eq!(h.command(actor, &hide_key), command);
    assert_eq!(optional_snapshot(&h.url, actor.id), scope);
    h.cleanup();
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn preferences_keep_origin_csrf_live_acl_and_work_with_operator_collection_off() {
    let h = Harness::new(false).await;
    let actor = &h.actor;
    let hide = choice(h.book, true, 0);
    let key = generate_session_token();
    private(
        &h.feedback(actor, &hide, &key, ("https://wrong.invalid", &actor.csrf))
            .await,
        StatusCode::FORBIDDEN,
    );
    private(
        &h.feedback(actor, &hide, &key, (ORIGIN, &generate_session_token()))
            .await,
        StatusCode::FORBIDDEN,
    );
    private(
        &h.send(
            "POST",
            FEEDBACK,
            &format!("tor-dev-csrf={}", actor.csrf),
            (ORIGIN, &actor.csrf),
            &hide.to_string(),
            Some(&key),
        )
        .await,
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        h.sql(&format!(
            "SELECT count(*) FROM rec_book_preferences WHERE user_id={}",
            actor.id
        )),
        "0"
    );
    receipt(
        h.feedback(actor, &hide, &key, (ORIGIN, &actor.csrf)).await,
        1,
        true,
    )
    .await;
    let settings = h.get(actor, "/me/recommendations").await;
    private(&settings, StatusCode::OK);
    assert!(text(settings).await.contains("Вернуть в рекомендации"));
    h.assert_no_analytics_subject(actor);

    h.sql(&format!(
        "UPDATE books SET admin_hold=true WHERE id={}",
        h.book
    ));
    let hidden = h.get(actor, FEEDBACK).await;
    private(&hidden, StatusCode::OK);
    assert_eq!(json(hidden).await, json!([]));
    let settings = text(h.get(actor, "/me/recommendations").await).await;
    assert!(!settings.contains("Preference &amp;"));
    private(
        &h.feedback(
            &h.other,
            &hide,
            &generate_session_token(),
            (ORIGIN, &h.other.csrf),
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    // Clearing an already chosen preference is still allowed after visibility
    // is revoked, without disclosing the now hidden book's title.
    receipt(
        h.feedback(
            actor,
            &choice(h.book, false, 1),
            &generate_session_token(),
            (ORIGIN, &actor.csrf),
        )
        .await,
        2,
        false,
    )
    .await;
    h.sql(&format!(
        "UPDATE users SET account_state='suspended' WHERE id={}",
        actor.id
    ));
    private(&h.get(actor, FEEDBACK).await, StatusCode::UNAUTHORIZED);
    private(
        &h.feedback(
            actor,
            &choice(h.book, true, 2),
            &generate_session_token(),
            (ORIGIN, &actor.csrf),
        )
        .await,
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(h.preference(actor)["hidden"], false);
    h.assert_no_analytics_subject(actor);
    h.assert_no_analytics_subject(&h.other);
    h.cleanup();
}
