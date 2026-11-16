//! C/S006 R1: a committed HTTP progress update cannot be lost or replayed
//! when an intermediate TCP proxy fails before acknowledging the response.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) async fn committed_cursor_survives_lost_ack(
    app: &Router,
    url: &str,
    sync_get: &str,
    sync_put: &str,
    reader_token: &str,
    csrf: &str,
    json: &str,
    user_id: i64,
    chapter_id: i64,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let wire = format!(
        "PUT {sync_put} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nOrigin: {ORIGIN}\r\nCookie: tor-dev-session={reader_token}; tor-dev-csrf={csrf}\r\nX-CSRF-Token: {csrf}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
        json.len()
    );

    // The test-only proxy refuses to acknowledge a successful COMMIT:
    // it accepts the exact authenticated HTTP PUT, routes it through Axum,
    // verifies the actual 200/revision=2, and closes TCP without transmitting
    // status, JSON receipt or any other response bytes.
    let routed = app.clone();
    let payload = json.to_owned();
    let token = reader_token.to_owned();
    let csrf = csrf.to_owned();
    let path = sync_put.to_owned();
    let expected_wire = wire.clone();
    let proxy = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = vec![0; expected_wire.len()];
        socket.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected_wire.as_bytes());
        let (status, receipt) =
            send(&routed, "PUT", &path, &payload, Some(&token), &csrf, ORIGIN).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            receipt.contains("\"revision\":2"),
            "the HTTP handler must commit before the proxy closes the socket"
        );
        socket.shutdown().await.unwrap();
    });
    let client = tokio::spawn(async move {
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        socket.write_all(wire.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        assert!(
            response.is_empty(),
            "client must receive zero bytes after a committed progress update"
        );
    });
    let (client_result, proxy_result) =
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(client, proxy)
        })
        .await
        .expect("progress lost-ack transport test must finish");
    client_result.expect("TCP reader");
    proxy_result.expect("committed progress proxy");

    // Independent PostgreSQL proof and fresh HTTP GET show revision 2 was
    // committed exactly once, with the new position rather than a stale one.
    let durable = pg(
        url,
        &format!(
            "SELECT revision || '|' || block_index || '|' || char_offset
             FROM reader_book_positions
             WHERE user_id={user_id} AND chapter_id={chapter_id}"
        ),
    );
    assert_eq!(durable, "2|1|2");
    let (status, state) = send(app, "GET", sync_get, "", Some(reader_token), &csrf, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.contains("\"revision\":2"));
    assert!(state.contains("\"char_offset\":2"));

    // A stale retry must fail with 409, never increment the revision or mint
    // completion/outbox events. The client resolves ambiguity by reloading
    // the saved state, not by assuming the lost response means rollback.
    let (status, _) = send(
        app,
        "PUT",
        sync_put,
        json,
        Some(reader_token),
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT revision || '|' || block_index || '|' || char_offset
                 FROM reader_book_positions
                 WHERE user_id={user_id} AND chapter_id={chapter_id}"
            )
        ),
        "2|1|2"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions
                 WHERE user_id={user_id} AND chapter_id={chapter_id}"
            )
        ),
        "0",
        "stale retry cannot fabricate a completed chapter"
    );
}
