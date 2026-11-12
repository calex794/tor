use super::ORIGIN;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request},
    response::Response,
    Router,
};
use std::process::Command;
use tower::ServiceExt;

pub(super) fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production DB forbidden");
    let result = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .output()
        .expect("psql client");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

pub(super) async fn call(
    app: &Router,
    method: &str,
    url: &str,
    cookie: &str,
    body: &str,
) -> Response {
    let mut req = Request::builder()
        .method(method)
        .uri(url)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            41001,
        ))));
    app.clone().oneshot(req).await.unwrap()
}

pub(super) async fn json_call(
    app: &Router,
    method: &str,
    url: &str,
    cookie: &str,
    csrf: &str,
    body: &str,
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(url)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_owned()))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            41002,
        ))));
    app.clone().oneshot(request).await.unwrap()
}

pub(super) fn auth_cookie(response: &Response, prefix: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(prefix))
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        .expect("auth Set-Cookie")
}

pub(super) async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
