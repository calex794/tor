//! Wire-level Yandex Cloud CDN contract against loopback only.
use super::{PurgeState, YandexCdnPurge};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Exchange {
    method: &'static str,
    path: &'static str,
    status: u16,
    body: &'static str,
}

async fn local_api(exact_path: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let cases = [
            Exchange { method: "POST", path: "/cdn/v1/cache/res-1234:purge", status: 200,
                body: r#"{"id":"op-1","done":false}"# },
            Exchange { method: "GET", path: "/operations/op-1", status: 200,
                body: r#"{"id":"op-1","done":false}"# },
            Exchange { method: "GET", path: "/operations/op-1", status: 200,
                body: r#"{"id":"op-1","done":true,"response":{}}"# },
            Exchange { method: "GET", path: "/operations/op-1", status: 200,
                body: r#"{"id":"op-1","done":true,"error":{"code":13}}"# },
            Exchange { method: "POST", path: "/cdn/v1/cache/res-1234:purge", status: 403,
                body: r#"{"message":"access denied"}"# },
        ];
        for item in cases {
            let (mut stream, peer) = listener.accept().await.unwrap();
            assert!(peer.ip().is_loopback());
            let mut request = Vec::new();
            let boundary = loop {
                let mut chunk = [0_u8; 2048];
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0, "incomplete CDN HTTP request");
                request.extend_from_slice(&chunk[..count]);
                if let Some(index) = request.windows(4).position(|v| v == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..index]);
                    let length = head.lines()
                        .find(|v| v.to_ascii_lowercase().starts_with("content-length:"))
                        .map(|v| v.split_once(':').unwrap().1.trim().parse::<usize>().unwrap())
                        .unwrap_or(0);
                    if request.len() >= index + 4 + length {
                        break index;
                    }
                }
                assert!(request.len() < 64 * 1024);
            };
            let head = String::from_utf8_lossy(&request[..boundary]);
            assert_eq!(head.lines().next().unwrap(),
                format!("{} {} HTTP/1.1", item.method, item.path));
            let authorization = head.lines()
                .find(|v| v.to_ascii_lowercase().starts_with("authorization:"))
                .unwrap();
            assert_eq!(authorization.split_once(':').unwrap().1.trim(), "Bearer mock-secret-123");
            if item.method == "POST" {
                let payload: serde_json::Value =
                    serde_json::from_slice(&request[boundary + 4..]).unwrap();
                assert_eq!(payload, serde_json::json!({"paths": [exact_path.as_str()]}),
                    "empty paths would purge the whole CDN");
            }
            let reason = if item.status == 200 { "OK" } else { "Forbidden" };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                item.status, reason, item.body.len(), item.body);
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (endpoint, task)
}

#[tokio::test]
async fn sends_only_exact_cover_path_and_polls_operation_before_acknowledgement() {
    let path = format!("/public/covers/950/7/card-{}.webp*", "ab".repeat(32));
    let (endpoint, server) = local_api(path.clone()).await;
    let mut api = YandexCdnPurge::new("res-1234".into(), "mock-secret-123".into()).unwrap();
    // These overrides compile only in unit tests; production uses fixed Yandex HTTPS hosts.
    api.test_cdn_endpoint = Some(endpoint.clone());
    api.test_operation_endpoint = Some(endpoint);

    assert!(api.submit("/public/covers/*").await.is_err());
    assert_eq!(api.submit(&path).await.unwrap(), "op-1");
    assert_eq!(api.poll("op-1").await.unwrap(), PurgeState::Running);
    assert_eq!(api.poll("op-1").await.unwrap(), PurgeState::Succeeded);
    assert_eq!(api.poll("op-1").await.unwrap(), PurgeState::Failed);
    assert!(api.submit(&path).await.is_err(), "HTTP 403 must fail closed");
    server.await.unwrap();
}
