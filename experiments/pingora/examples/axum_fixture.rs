//! Synthetic public catalogue and reading pages for proxy-only load tests.
//! No database, user data, paid grants, cookies or external services.
use axum::{
    http::{header, StatusCode},
    response::Html,
    routing::get,
    Router,
};
use std::sync::LazyLock;

static CATALOG: LazyLock<String> = LazyLock::new(|| {
    let card =
        "<article class='book'><h2>Example book</h2><p>Author, genre, language</p></article>";
    format!(
        "<!doctype html><html lang='ru'><head><title>Catalog fixture</title></head><body><main>{}</main></body></html>",
        card.repeat(48)
    )
});

static READING: LazyLock<String> = LazyLock::new(|| {
    let paragraph = "<p>Example public chapter text used for repeatable HTTP measurements.</p>";
    format!(
        "<!doctype html><html lang='ru'><head><title>Reader fixture</title></head><body><article>{}</article></body></html>",
        paragraph.repeat(160)
    )
});

async fn catalogue() -> Html<&'static str> {
    Html(CATALOG.as_str())
}

async fn reader() -> Html<&'static str> {
    Html(READING.as_str())
}

async fn books_api() -> ([(header::HeaderName, &'static str); 1], &'static str) {
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        r#"{"books":[{"id":1,"title":"Example book","language":"ru","is_public":true},{"id":2,"title":"Another example","language":"ru","is_public":true}]}"#,
    )
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html("<!doctype html><html><body>Public fixture</body></html>") }),
        )
        .route("/catalog", get(catalogue))
        .route("/books/1", get(reader))
        .route("/api/v1/books", get(books_api))
        .route(
            "/status/429",
            get(|| async { StatusCode::TOO_MANY_REQUESTS }),
        )
        .route(
            "/status/503",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:6194")
        .await
        .expect("fixture must bind to loopback");
    println!("PINGORA_FIXTURE_ONLY 127.0.0.1:6194");
    axum::serve(listener, app)
        .await
        .expect("fixture HTTP server");
}
