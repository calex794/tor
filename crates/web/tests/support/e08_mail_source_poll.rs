//! Source-compatible GET /mail/{id}/messages: actor scope, cursors, safe JSON.
//! Called by the existing disposable /tor_ci mail HTTP scenario.
use super::{call, text};
use axum::{
    http::{header, StatusCode},
    Router,
};

pub(super) async fn verify(app: &Router, id: i64, cookies: &[String]) {
    let path = format!("/mail/{id}/messages");
    assert_eq!(
        call(app, "GET", &path, "", "", &[]).await.status(),
        StatusCode::UNAUTHORIZED,
    );
    assert_eq!(
        call(app, "GET", &path, &cookies[2], "", &[]).await.status(),
        StatusCode::NOT_FOUND,
        "unrelated user must not discover a private conversation",
    );
    let response = call(
        app,
        "GET",
        &format!("{path}?after_id=0&limit=50"),
        &cookies[1],
        "",
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("no-store"));
    let value: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["messages"].as_array().unwrap().len(), 1);
    let message = &value["messages"][0];
    assert_eq!(message["text"], "PRIVATE\n<script> /books/2");
    assert!(message["body_without_book_links"].is_string());
    assert!(message["book_previews"].is_array());
    assert!(value["has_more_older"].is_boolean());
    let message_id = message["id"].as_str().unwrap();
    assert_eq!(value["oldest_message_id"].as_str(), Some(message_id));
    assert_eq!(value["newest_message_id"].as_str(), Some(message_id));

    let next = call(
        app,
        "GET",
        &format!("{path}?after_id={message_id}&limit=50"),
        &cookies[1],
        "",
        &[],
    )
    .await;
    assert_eq!(next.status(), StatusCode::OK);
    let empty: serde_json::Value = serde_json::from_str(&text(next).await).unwrap();
    assert!(empty["messages"].as_array().unwrap().is_empty());

    let older = call(
        app,
        "GET",
        &format!(
            "{path}?before_id={}&limit=50",
            id_from_string(message_id) + 1
        ),
        &cookies[0],
        "",
        &[],
    )
    .await;
    assert_eq!(older.status(), StatusCode::OK);
    let older_page: serde_json::Value = serde_json::from_str(&text(older).await).unwrap();
    assert_eq!(older_page["messages"].as_array().unwrap().len(), 1);

    assert_eq!(
        call(
            app,
            "GET",
            &format!("{path}?before_id=1&after_id=0"),
            &cookies[0],
            "",
            &[],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{path}?actor_id=123"),
            &cookies[0],
            "",
            &[],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST,
    );
}

fn id_from_string(value: &str) -> i64 {
    value.parse().expect("numeric message id")
}
