//! E-08 legacy Book ref regression: only explicitly mapped aliases, bounded
//! canonical public/owner title access, and no hidden/held leakage to peers.
use super::{call, pg, text, ORIGIN};
use axum::{http::StatusCode, Router};

pub(super) async fn verify(
    app: &Router,
    db_url: &str,
    path: &str,
    nonce: u128,
    books: [i64; 3],
    cookies: &[String],
    csrf: &[String],
) {
    let [public, private, held] = books;
    let public_alias = format!("legacy-public-{nonce}");
    let private_alias = format!("legacy-private-{nonce}");
    let held_alias = format!("legacy-held-{nonce}");
    let unknown_alias = format!("legacy-unknown-{nonce}");
    pg(
        db_url,
        &format!(
            "INSERT INTO book_legacy_mail_refs(alias_key,book_id) VALUES
             ('{public_alias}',{public}),
             ('{private_alias}',{private}),
             ('{held_alias}',{held})",
        ),
    )
    .await;

    let form = format!(
        "body=Links%20%2Fbooks%2F{public_alias}%20%2Fbook%2F{private_alias}%20%2Fbooks%2F{held_alias}%20%2Fbooks%2F{unknown_alias}&client_message_id=legacy-{nonce}&csrf_token={}",
        csrf[0]
    );
    let sent = call(
        app,
        "POST",
        &format!("{path}/messages"),
        &cookies[0],
        &form,
        &[
            ("accept", "application/json"),
            ("x-requested-with", "fetch"),
        ],
    )
    .await;
    assert_eq!(sent.status(), StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_str(&text(sent).await).unwrap();
    let message = &payload["message"];
    let previews = message["book_previews"].as_array().unwrap();
    assert_eq!(previews.len(), 2);
    assert_eq!(previews[0]["url"], format!("/books/{public}"));
    assert_eq!(previews[1]["url"], format!("/me/books/{private}"));
    assert_eq!(previews[1]["cover_url"], "");
    assert!(!message.to_string().contains("HELD-NEVER-DISCLOSE"));
    let body = message["body_without_book_links"].as_str().unwrap();
    assert!(body.contains(&format!("/books/{held_alias}")));
    assert!(body.contains(&format!("/books/{unknown_alias}")));
    assert!(!body.contains(&public_alias));
    assert!(!body.contains(&private_alias));

    let conversation_id: i64 = path.trim_start_matches("/mail/").parse().unwrap();
    let message_id: i64 = message["id"].as_str().unwrap().parse().unwrap();
    let peer = call(
        app,
        "GET",
        &format!(
            "/api/v1/me/messages?conversation_id={conversation_id}&after_id={}",
            message_id - 1
        ),
        &cookies[1],
        "",
        &[],
    )
    .await;
    assert_eq!(peer.status(), StatusCode::OK);
    let peer_json: serde_json::Value = serde_json::from_str(&text(peer).await).unwrap();
    let item = &peer_json["messages"][0];
    assert_eq!(item["book_previews"].as_array().unwrap().len(), 1);
    assert_eq!(item["book_previews"][0]["url"], format!("/books/{public}"));
    assert!(!item.to_string().contains("HIDDEN-NEVER-DISCLOSE"));
    assert!(!item.to_string().contains("HELD-NEVER-DISCLOSE"));
    let peer_body = item["body_without_book_links"].as_str().unwrap();
    assert!(peer_body.contains(&format!("/book/{private_alias}")));
    assert!(peer_body.contains(&format!("/books/{held_alias}")));

    // Imported aliases are globally one-to-one. No second Book may claim
    // the same legacy ref and silently reassign previous mail links.
    // A contained PL/pgSQL exception verifies SQLSTATE 23505. If the
    // duplicate insert unexpectedly succeeds, the block raises a different
    // exception and the disposable tor_ci fixture fails closed.
    pg(
        db_url,
        &format!(
            "DO $e08$
             BEGIN
               BEGIN
                 INSERT INTO book_legacy_mail_refs(alias_key,book_id)
                 VALUES('{public_alias}',{private});
                 RAISE EXCEPTION 'legacy alias uniqueness fence failed';
               EXCEPTION WHEN unique_violation THEN NULL;
               END;
             END $e08$;"
        ),
    )
    .await;

    // A live ACL change must revoke an already sent legacy preview.
    // B-10 forbids deactivating an owner with current revenue shares, so
    // exercise the Book moderation hold rather than bypassing that fence.
    // The explicit alias mapping remains intact throughout this check.
    pg(
        db_url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={private}"),
    )
    .await;
    let own_poll = call(
        app,
        "GET",
        &format!(
            "/api/v1/me/messages?conversation_id={conversation_id}&after_id={}",
            message_id - 1
        ),
        &cookies[0],
        "",
        &[],
    )
    .await;
    let own_json: serde_json::Value = serde_json::from_str(&text(own_poll).await).unwrap();
    let after_revoke = &own_json["messages"][0];
    assert_eq!(after_revoke["book_previews"].as_array().unwrap().len(), 1);
    assert_eq!(
        after_revoke["book_previews"][0]["url"],
        format!("/books/{public}")
    );
    assert!(after_revoke["body_without_book_links"]
        .as_str()
        .unwrap()
        .contains(&format!("/book/{private_alias}")));

    // A syntactically valid but unmapped legacy string is never looked up
    // against books.source_identity or the hidden Book catalog.
    let unmapped = call(
        app,
        "POST",
        &format!("{path}/messages"),
        &cookies[0],
        &format!(
            "body=Unmapped%20%2Fbooks%2F{unknown_alias}&client_message_id=unmapped-{nonce}&csrf_token={}",
            csrf[0]
        ),
        &[
            ("accept", "application/json"),
            ("x-requested-with", "fetch"),
        ],
    )
    .await;
    assert_eq!(unmapped.status(), StatusCode::OK);
    let unmapped_json: serde_json::Value = serde_json::from_str(&text(unmapped).await).unwrap();
    let unmapped_message = &unmapped_json["message"];
    assert!(unmapped_message["book_previews"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(unmapped_message["body_without_book_links"]
        .as_str()
        .unwrap()
        .contains(&format!("/books/{unknown_alias}")));

    // A foreign absolute URL must remain visible as sent, even if it ends
    // with a known public Book ID. A configured-origin URL may be enriched.
    let trusted_origin = ORIGIN.replace(':', "%3A").replace('/', "%2F");
    let foreign = format!("https://evil.test/books/{public}?ref=mail");
    let host_form = format!(
        "body=Foreign%20https%3A%2F%2Fevil.test%2Fbooks%2F{public}%3Fref%3Dmail%20and%20trusted%20{trusted_origin}%2Fbooks%2F{public}&client_message_id=host-check-{nonce}&csrf_token={}",
        csrf[0]
    );
    let host_response = call(
        app,
        "POST",
        &format!("{path}/messages"),
        &cookies[0],
        &host_form,
        &[
            ("accept", "application/json"),
            ("x-requested-with", "fetch"),
        ],
    )
    .await;
    assert_eq!(host_response.status(), StatusCode::OK);
    let host_json: serde_json::Value = serde_json::from_str(&text(host_response).await).unwrap();
    let host_view = &host_json["message"];
    assert_eq!(host_view["book_previews"].as_array().unwrap().len(), 1);
    assert_eq!(
        host_view["book_previews"][0]["url"],
        format!("/books/{public}")
    );
    let plain = host_view["body_without_book_links"].as_str().unwrap();
    assert!(plain.contains(&foreign));
    assert!(!plain.contains(&format!("{ORIGIN}/books/{public}")));
}
