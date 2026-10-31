//! E-08 safe notification Open action, mounted by the disposable mail HTTP test.
use super::{call, text};
use axum::{
    http::{header, StatusCode},
    Router,
};

pub(super) async fn verify(
    app: &Router,
    recipient_cookie: &str,
    recipient_csrf: &str,
    notification_id: i64,
    target: &str,
) {
    let page = text(call(app, "GET", "/notifications", recipient_cookie, "", &[]).await).await;
    assert!(page.contains(&format!(
        "action=\"/notifications/{notification_id}/read\""
    )));
    assert!(page.contains(&format!("name=\"next\" value=\"{target}\"")));
    assert!(page.contains(">Открыть</button>"));

    let route = format!("/notifications/{notification_id}/read");
    let encoded_target = target.replace('/', "%2F");
    let open = call(
        app,
        "POST",
        &route,
        recipient_cookie,
        &format!("csrf_token={recipient_csrf}&next={encoded_target}"),
        &[],
    )
    .await;
    assert_eq!(open.status(), StatusCode::SEE_OTHER);
    assert_eq!(open.headers()[header::LOCATION], target);

    // Cross-origin, scheme-relative, backslash, API and dot segments are not
    // redirect targets, even for a signed-in actor who owns the notification.
    for hostile in [
        "https%3A%2F%2Fattacker.invalid%2Fsteal",
        "%2F%2Fattacker.invalid",
        "%2F%5Cattacker.invalid",
        "%2Fapi%2Fv1%2Fme",
        "%2F..%2Fmail%2F123",
    ] {
        let response = call(
            app,
            "POST",
            &route,
            recipient_cookie,
            &format!("csrf_token={recipient_csrf}&next={hostile}"),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/notifications");
    }
}
