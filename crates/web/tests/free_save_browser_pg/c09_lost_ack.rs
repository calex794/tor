//! C/S006 R1: prove the real TCP/Axum/PG commit boundary when a forwarding
//! proxy receives the successful HTTP response but delivers zero bytes to
//! the reader. This is not a direct in-process Axum handler shortcut.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) async fn committed_cursor_survives_lost_ack(
    app: &Router,
    url: &str,
    reader_token: &str,
    csrf: &str,
    json: &str,
    user_id: i64,
    chapter_id: i64,
) {
    let sync_get = format!("/api/v1/me/progress/free?chapter_id={chapter_id}");
    let sync_put = "/api/v1/me/progress/free";
    // Use a real HTTP listener: the proxy forwards actual TCP bytes into
    // Axum instead of calling a cloned Router directly.
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    let backend_app = app.clone();
    let backend_server = tokio::spawn(async move {
        axum::serve(backend_listener, backend_app)
            .await
            .expect("real HTTP progress server");
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let wire = format!(
        "PUT {sync_put} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nOrigin: {ORIGIN}\r\nCookie: tor-dev-session={reader_token}; tor-dev-csrf={csrf}\r\nX-CSRF-Token: {csrf}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
        json.len()
    );

    // The proxy receives the exact authenticated wire request, forwards it
    // unchanged into a live Axum TCP server, waits for a complete HTTP 200,
    // and deliberately discards every response byte before closing the
    // reader-facing connection. No handler call bypasses the network stack.
    let expected_wire = wire.clone();
    let proxy = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = vec![0; expected_wire.len()];
        socket.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected_wire.as_bytes());

        let mut upstream = tokio::net::TcpStream::connect(backend_addr)
            .await
            .expect("forward to live Axum");
        upstream
            .write_all(&received)
            .await
            .expect("forward full authenticated PUT");
        let mut reply = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            upstream.read_to_end(&mut reply),
        )
        .await
        .expect("live HTTP reply must finish")
        .expect("read committed HTTP reply");
        assert!(
            reply.starts_with(b"HTTP/1.1 200 "),
            "the live HTTP server must acknowledge the committed PUT"
        );
        let receipt = String::from_utf8_lossy(&reply);
        assert!(receipt.contains("\"revision\":2"));
        assert!(receipt.contains("\"char_offset\":2"));
        // The socket sent to the reader never receives this reply.
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
    backend_server.abort();

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
    let (status, state) = send(app, "GET", &sync_get, "", Some(reader_token), csrf, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.contains("\"revision\":2"));
    assert!(state.contains("\"char_offset\":2"));

    // A stale retry must fail with 409, never increment the revision or mint
    // completion/outbox events. The client resolves ambiguity by reloading
    // the saved state, not by assuming the lost response means rollback.
    let (status, _) = send(app, "PUT", sync_put, json, Some(reader_token), csrf, ORIGIN).await;
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
