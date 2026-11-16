//! D/S031 R1: a Layer checkout COMMIT succeeds over live Axum HTTP, while
//! a test-only forwarding proxy deliberately loses the complete HTTP receipt.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) async fn drop_committed_layer_receipt(
    app: &Router,
    path: &str,
    payload: &str,
    buyer_token: &str,
    csrf: &str,
    key: &str,
) -> String {
    let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = backend_listener.local_addr().unwrap();
    let backend_app = app.clone();
    let backend_server = tokio::spawn(async move {
        axum::serve(backend_listener, backend_app)
            .await
            .expect("real Layer checkout HTTP server");
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = listener.local_addr().unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {proxy_addr}\r\nContent-Type: application/json\r\nOrigin: {ORIGIN}\r\nCookie: tor-dev-session={buyer_token}; tor-dev-csrf={csrf}\r\nX-CSRF-Token: {csrf}\r\nIdempotency-Key: {key}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let expected = request.clone();

    let proxy = tokio::spawn(async move {
        let (mut downstream, _) = listener.accept().await.unwrap();
        let mut received = vec![0_u8; expected.len()];
        downstream.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected.as_bytes(), "authenticated HTTP request mismatch");

        let mut upstream = tokio::net::TcpStream::connect(backend)
            .await
            .expect("connect to live Axum");
        upstream
            .write_all(&received)
            .await
            .expect("forward exact HTTP request to Axum");
        let mut reply = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(15),
            upstream.read_to_end(&mut reply),
        )
        .await
        .expect("Layer checkout HTTP response timeout")
        .expect("read full live Layer response");
        assert!(reply.starts_with(b"HTTP/1.1 200 "), "committed checkout must return HTTP 200");
        let receipt = String::from_utf8_lossy(&reply);
        assert!(receipt.contains("\"balance_minor\":120"), "renewal must debit once");
        assert!(receipt.contains("\"replayed\":false"), "original renewal must not be replayed");
        assert!(receipt.contains("\"term_id\""), "live receipt must identify committed entitlement");
        // Do not forward any response bytes to the reader.
        downstream.shutdown().await.unwrap();
        receipt.into_owned()
    });
    let client = tokio::spawn(async move {
        let mut downstream = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
        downstream.write_all(request.as_bytes()).await.unwrap();
        let mut received = Vec::new();
        downstream.read_to_end(&mut received).await.unwrap();
        assert!(received.is_empty(), "reader must see zero HTTP acknowledgement bytes");
    });
    let (client_result, proxy_result) =
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(client, proxy)
        })
        .await
        .expect("test-only proxy must terminate");
    client_result.expect("reader TCP client");
    let wire_receipt = proxy_result.expect("server completed renewal and discarded reply");
    backend_server.abort();
    wire_receipt
}
