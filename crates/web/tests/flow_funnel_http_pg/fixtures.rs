//! Real account forms, loopback-only SQL fixtures, and HTTP cookie jar.
use super::*;

pub(super) fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let output = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
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
            statement,
        ])
        .output()
        .expect("disposable PostgreSQL client");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub(super) async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[derive(Default)]
pub(super) struct Browser {
    pub(super) cookies: BTreeMap<String, String>,
}

impl Browser {
    pub(super) fn accept(&mut self, response: &Response) {
        for cookie in response.headers().get_all(header::SET_COOKIE) {
            let cookie = cookie.to_str().unwrap().split(';').next().unwrap();
            let (name, value) = cookie.split_once('=').unwrap();
            self.cookies.insert(name.to_owned(), value.to_owned());
        }
    }

    pub(super) fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub(super) async fn request(
        &self,
        app: &Router,
        method: &str,
        path: &str,
        body: &str,
        extra: &[(&str, &str)],
    ) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                43198,
            ))))
            .header(header::COOKIE, self.cookie_header())
            .header(header::HOST, "127.0.0.1:3098")
            .header(header::USER_AGENT, ORDINARY_UA)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        for (name, value) in extra {
            request.headers_mut().unwrap().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        app.clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    pub(super) async fn get(&self, app: &Router, path: &str) -> Response {
        self.request(app, "GET", path, "", &[]).await
    }
}

pub(super) async fn registration(app: &Router, url: &str, email: &str) -> (Browser, i64) {
    let mut browser = Browser::default();
    let page = browser.get(app, "/register").await;
    assert_eq!(page.status(), StatusCode::OK);
    browser.accept(&page);
    let body = format!(
        "email={email}&display_name=FLOW+test&password=FlowVisitsPassword2026&csrf_token={}",
        browser.cookies["tor-dev-csrf"]
    );
    let response = browser.request(app, "POST", "/register", &body, &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    browser.accept(&response);
    let id = pg(
        url,
        &format!("SELECT id FROM users WHERE email_normalized='{email}'"),
    )
    .parse()
    .unwrap();
    (browser, id)
}

pub(super) async fn login(app: &Router, email: &str) -> Browser {
    let mut browser = Browser::default();
    let page = browser.get(app, "/login").await;
    assert_eq!(page.status(), StatusCode::OK);
    browser.accept(&page);
    let body = format!(
        "email={email}&password=FlowVisitsPassword2026&csrf_token={}",
        browser.cookies["tor-dev-csrf"]
    );
    let response = browser.request(app, "POST", "/login", &body, &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    browser.accept(&response);
    browser
}

pub(super) async fn settings(browser: &Browser, app: &Router) -> String {
    let response = browser.get(app, "/me/recommendations").await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await
}

pub(super) async fn retired_choice(browser: &Browser, app: &Router, action: &str, enabled: bool) {
    let body = format!(
        "enabled={}&expected_revision=7&command_key={}&csrf_token={}",
        if enabled { "1" } else { "" },
        generate_session_token(),
        browser.cookies["tor-dev-csrf"]
    );
    let response = browser.request(app, "POST", action, &body, &[]).await;
    assert_eq!(response.status(), StatusCode::GONE);
    private(&response);
    let body: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(body["error"]["code"], "COLLECTION_RETIRED");
}

pub(super) fn private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
}

pub(super) fn own_pages(url: &str, actor: i64) -> i64 {
    pg(
        url,
        &format!(
            "SELECT COALESCE(sum(v.pageviews),0) FROM flow_visits v
        JOIN rec_subjects s ON s.subject_id=v.subject_id WHERE s.user_id={actor}"
        ),
    )
    .parse()
    .unwrap()
}
