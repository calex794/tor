//! End-to-end S025: real PG/MinIO cleanup worker and real Yandex HTTP client
//! against a strict loopback provider. No cloud provider or production token.
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tor_books::repositories::cover_slots::PublicCoverKind;

pub(super) async fn start(
    public_urls: &[(PublicCoverKind, String)],
) -> (String, tokio::task::JoinHandle<()>) {
    assert_eq!(public_urls.len(), 2);
    let exact_paths: Vec<String> = public_urls
        .iter()
        .map(|(_, url)| {
            format!(
                "{}*",
                url.strip_prefix("https://cdn.example")
                    .expect("only test CDN URL")
            )
        })
        .collect();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        // Both variants are removed from MinIO before any POST. Polling reports
        // two pending operations, a failure + success, then one exact retry.
        for step in 0..8 {
            let operation = match step {
                0 | 2 | 4 => "ci-operation-0",
                1 | 3 | 5 => "ci-operation-1",
                6 | 7 => "ci-operation-2",
                _ => unreachable!(),
            };
            let method = if matches!(step, 0 | 1 | 6) {
                "POST"
            } else {
                "GET"
            };
            let route = if method == "POST" {
                "/cdn/v1/cache/res-1234:purge".to_owned()
            } else {
                format!("/operations/{operation}")
            };
            let outcome = match step {
                4 => "failed",
                5 | 7 => "success",
                _ => "pending",
            };
            let file_index = match step {
                0 | 6 => Some(0),
                1 => Some(1),
                _ => None,
            };
            let (mut stream, peer) = listener.accept().await.unwrap();
            assert!(peer.ip().is_loopback(), "mock may accept only loopback");
            let mut request = Vec::new();
            let boundary = loop {
                let mut chunk = [0u8; 2048];
                let size = stream.read(&mut chunk).await.unwrap();
                assert!(size > 0, "incomplete loopback request");
                request.extend_from_slice(&chunk[..size]);
                assert!(request.len() <= 64 * 1024, "oversized API request");
                if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..index]);
                    let length = headers
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
                        .map(|line| {
                            line.split_once(':')
                                .unwrap()
                                .1
                                .trim()
                                .parse::<usize>()
                                .unwrap()
                        })
                        .unwrap_or(0);
                    if request.len() >= index + 4 + length {
                        break index;
                    }
                }
            };
            let head = String::from_utf8_lossy(&request[..boundary]);
            assert_eq!(
                head.lines().next().unwrap(),
                format!("{method} {route} HTTP/1.1"),
                "Yandex client must use its actual management API route"
            );
            let auth = head
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                .unwrap();
            assert_eq!(
                auth.split_once(':').unwrap().1.trim(),
                "Bearer mock-secret-123"
            );
            if let Some(index) = file_index {
                let payload: serde_json::Value =
                    serde_json::from_slice(&request[boundary + 4..]).unwrap();
                assert_eq!(
                    payload,
                    serde_json::json!({"paths": [exact_paths[index].as_str()]}),
                    "no empty or broad purge; retries must target exactly one old cover"
                );
            }
            let body = match outcome {
                "pending" => serde_json::json!({"id":operation,"done":false}),
                "failed" => serde_json::json!({"id":operation,"done":true,"error":{"code":13}}),
                "success" => serde_json::json!({"id":operation,"done":true,"response":{}}),
                _ => unreachable!(),
            }
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (endpoint, task)
}
