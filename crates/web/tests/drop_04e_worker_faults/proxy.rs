//! CI-only HTTP fault relay. Forwards signed bytes to the real loopback MinIO.
//! It never logs authorization, URLs or body data, and has no production wiring.
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug)]
pub enum Fault {
    BeforePut,
    PackPutAck,
    ManifestPutAck,
    FirstGet,
    SecondGet,
    CorruptGet,
    StealLease,
    HoldAfterStage,
}

struct StateData {
    fault: Fault,
    http: reqwest::Client,
    gets: AtomicUsize,
    hits: AtomicUsize,
    database: String,
    job: i64,
    book: i64,
}
pub struct Relay {
    state: Arc<StateData>,
    server: JoinHandle<()>,
    pub endpoint: String,
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Relay {
    pub async fn start(fault: Fault, database: &str, job: i64, book: i64) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(StateData {
            fault,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap(),
            gets: AtomicUsize::new(0),
            hits: AtomicUsize::new(0),
            database: database.into(),
            job,
            book,
        });
        let app = Router::new().fallback(relay).with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            server,
            endpoint,
        }
    }
    pub fn hits(&self) -> usize {
        self.state.hits.load(Ordering::SeqCst)
    }
}
fn unavailable() -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::CONTENT_TYPE, "application/xml")
        .body(Body::from(
            "<Error><Code>ServiceUnavailable</Code><Message>CI injected fault</Message></Error>",
        ))
        .unwrap()
}
async fn relay(State(state): State<Arc<StateData>>, request: Request<Body>) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    assert!(
        path.starts_with("/tor-drop-worker-ci/private/free-packs/"),
        "relay accepts destination PACK only"
    );
    let packed = path.ends_with(".zst");
    let put = parts.method == axum::http::Method::PUT;
    let get = parts.method == axum::http::Method::GET && packed;
    let number = if get {
        state.gets.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        0
    };
    let before = matches!(state.fault, Fault::BeforePut) && put
        || matches!(state.fault, Fault::FirstGet) && get && number == 1
        || matches!(state.fault, Fault::SecondGet) && get && number == 2;
    if before {
        state.hits.fetch_add(1, Ordering::SeqCst);
        return unavailable();
    }
    let bytes = to_bytes(body, 2 * 1024 * 1024).await.unwrap();
    let mut headers = parts.headers;
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::CONNECTION);
    // Preserve the signed original Host even though the TCP destination is
    // MinIO's port. No S3 key, query, payload or signed header is rewritten.
    let upstream = state
        .http
        .request(parts.method, format!("http://127.0.0.1:9000{}", parts.uri))
        .headers(headers)
        .body(bytes)
        .send()
        .await
        .unwrap();
    let status = upstream.status();
    let mut headers = upstream.headers().clone();
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::CONNECTION);
    let mut bytes = upstream.bytes().await.unwrap().to_vec();
    let lost_ack = put
        && status.is_success()
        && (matches!(state.fault, Fault::PackPutAck) && packed
            || matches!(state.fault, Fault::ManifestPutAck) && !packed);
    if lost_ack {
        state.hits.fetch_add(1, Ordering::SeqCst);
        return unavailable();
    }
    if get && number == 1 && matches!(state.fault, Fault::CorruptGet) {
        assert!(!bytes.is_empty());
        bytes[0] ^= 1;
        state.hits.fetch_add(1, Ordering::SeqCst);
    }
    if get && number == 1 && matches!(state.fault, Fault::StealLease) {
        super::support::pg(
            &state.database,
            &format!(
                "UPDATE chapter_import_jobs SET lease_epoch=lease_epoch+1 WHERE id={}",
                state.job
            ),
        );
        state.hits.fetch_add(1, Ordering::SeqCst);
    }
    if get && number == 2 && matches!(state.fault, Fault::HoldAfterStage) {
        super::support::pg(
            &state.database,
            &format!("UPDATE books SET admin_hold=true WHERE id={}", state.book),
        );
        state.hits.fetch_add(1, Ordering::SeqCst);
    }
    let mut response = Response::builder().status(status);
    *response.headers_mut().unwrap() = headers;
    response.body(Body::from(bytes)).unwrap()
}
