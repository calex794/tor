//! H/UI-08: owner list, shared navigation and consistent logout CSRF.
use super::*;

pub(super) async fn owner_index(app: &Router, session: &str, book_path: &str) {
    let guest = send(app, request("GET", "/me/books", "", "", ORIGIN)).await;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    assert_eq!(guest.headers()[header::LOCATION], "/login");
    let page = send(app, request("GET", "/me/books", "", session, ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let expected = csrf_cookie.split_once('=').unwrap().1;
    let html = body_of(page).await;
    assert!(html.contains(&format!("href=\"{book_path}\"")));
    assert!(html.contains("account-sidebar"));
    assert!(html.contains("href=\"/me/books\" aria-current=\"page\""));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    for token in html.split("name=\"csrf_token\" value=\"").skip(1) {
        assert_eq!(token.split('"').next().unwrap(), expected);
    }
    let reused = send(
        app,
        request(
            "GET",
            "/me/books",
            "",
            &format!("{session}; {csrf_cookie}"),
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(reused.status(), StatusCode::OK);
    assert!(reused.headers().get(header::SET_COOKIE).is_none());
}
