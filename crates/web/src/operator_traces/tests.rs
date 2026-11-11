use super::*;
use axum::body::to_bytes;

fn credentials(password: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!(
            "Basic {}",
            STANDARD.encode(format!("operator:{password}"))
        ))
        .unwrap(),
    );
    headers
}

#[tokio::test]
async fn operator_password_is_required_before_query_validation() {
    let secret = "a".repeat(48);
    for (headers, value) in [
        (HeaderMap::new(), Some(secret.as_str())),
        (credentials("wrong"), Some(secret.as_str())),
        (credentials(&secret), None),
        (credentials("short"), Some("short")),
    ] {
        let response = respond(headers, Some("id=<script>"), value, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    }
}

#[tokio::test]
async fn valid_password_yields_form_but_unavailable_store_is_not_a_missing_trace() {
    let secret = "a".repeat(48);
    let response = respond(credentials(&secret), None, Some(&secret), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("content-security-policy"));
    let response = respond(
        credentials(&secret),
        Some("id=00000000000000000000000000000010"),
        Some(&secret),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let contents = to_bytes(response.into_body(), 20_000).await.unwrap();
    let body = String::from_utf8(contents.to_vec()).unwrap();
    assert!(body.contains("не включена"));
    assert!(!body.contains("user_id"));
}

#[tokio::test]
async fn malformed_input_is_rejected_without_echo() {
    let secret = "a".repeat(48);
    for query in [
        "id=<script>",
        "id=1",
        "other=1",
        "id=11111111111111111111111111111111&role=admin",
    ] {
        let response = respond(credentials(&secret), Some(query), Some(&secret), None).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 20_000).await.unwrap();
        assert!(!String::from_utf8(body.to_vec())
            .unwrap()
            .contains("<script>"));
    }
}

#[test]
fn password_has_to_be_strong_ascii_and_a_single_header() {
    let secret = "a".repeat(48);
    let mut header = credentials(&secret);
    assert!(authorized(&header, Some(&secret)));
    header.append(
        header::AUTHORIZATION,
        HeaderValue::from_static("Basic Zm9vOmJhcg=="),
    );
    assert!(!authorized(&header, Some(&secret)));
    assert!(!authorized(&credentials(&secret), Some("short")));
    assert!(!authorized(&credentials(&secret), Some("💠")));
}

#[cfg(unix)]
#[tokio::test]
async fn authenticated_operator_reads_complete_root_and_sqlx_from_socket() {
    use std::{
        fs,
        io::{Read, Write},
        os::unix::net::UnixListener,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tor_observability::{
        domain::{RouteGroup, StatusGroup},
        services::{
            lookup::{self, LookupResult},
            trace::{RequestTrace, SqlEvidence, SqlSpan},
        },
    };

    let dir = std::env::temp_dir().join(format!(
        "tor-operator-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    let socket = dir.join("trace.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let worker = thread::spawn(move || {
        let (mut peer, _) = listener.accept().unwrap();
        let mut length = [0_u8; 4];
        peer.read_exact(&mut length).unwrap();
        assert_eq!(u32::from_be_bytes(length) as usize, lookup::QUERY_SIZE);
        let mut query = [0_u8; lookup::QUERY_SIZE];
        peer.read_exact(&mut query).unwrap();
        assert_eq!(lookup::parse_query(&query), Some(0xabcdef));
        let result = LookupResult {
            archived: 150,
            matches: 1,
            found: Some((
                1_900_000_000,
                RequestTrace {
                    trace_id: 0xabcdef,
                    route: RouteGroup::Reader,
                    status: StatusGroup::ServerError,
                    duration_us: 240_000,
                    sqlx: Some(SqlEvidence {
                        observed_queries: 1,
                        observed_elapsed_us: 175_000,
                        spans: vec![SqlSpan {
                            completed_at_us: 200_000,
                            elapsed_us: 175_000,
                        }],
                        uncaptured_spans: 0,
                    }),
                },
            )),
        };
        let reply = lookup::encode_reply(&result).unwrap();
        peer.write_all(&(reply.len() as u32).to_be_bytes()).unwrap();
        peer.write_all(&reply).unwrap();
    });
    let secret = "a".repeat(48);
    let response = respond(
        credentials(&secret),
        Some("id=00000000000000000000000000abcdef"),
        Some(&secret),
        Some(socket),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 50_000).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("00000000000000000000000000abcdef"));
    assert!(html.contains("150"));
    assert!(html.contains("PostgreSQL (SQLx)"));
    assert!(html.contains("175.00 мс"));
    assert!(!html.contains("user_id"));
    worker.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
}
