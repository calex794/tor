//! Actor-bound E-08 preferences HTTP regression, exercised from mail_http_pg.
use super::{call, text};
use axum::http::{header, StatusCode};
use axum::Router;
use tor_social::repositories::notifications::PgNotifications;

pub(super) async fn verify(
    app: &Router,
    recipient_cookie: &str,
    recipient_csrf: &str,
    recipient_id: i64,
    other_id: i64,
    notifications: &PgNotifications,
) {
    // Per-kind NEwBOO preferences must bind to the verified session actor.
    let settings_page =
        text(call(app, "GET", "/notifications", recipient_cookie, "", &[]).await).await;
    assert!(settings_page.contains("action=\"/notifications/preferences\""));
    assert!(settings_page.contains("name=\"pref_mail\" value=\"on\" checked"));
    assert_eq!(
        call(
            app,
            "POST",
            "/notifications/preferences",
            recipient_cookie,
            "csrf_token=forged&pref_friendship=on",
            &[],
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            app,
            "POST",
            "/notifications/preferences",
            recipient_cookie,
            &format!(
                "csrf_token={}&pref_friendship=on&user_id={}",
                recipient_csrf, other_id
            ),
            &[("origin", "https://evil.invalid")],
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let saved_preferences = call(
        app,
        "POST",
        "/notifications/preferences",
        recipient_cookie,
        &format!(
            "csrf_token={}&pref_friendship=on&user_id={}",
            recipient_csrf, other_id
        ),
        &[],
    )
    .await;
    assert_eq!(saved_preferences.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        saved_preferences.headers()[header::LOCATION],
        "/notifications?notice=preferences_saved"
    );
    let saved = notifications.preferences(recipient_id).await.unwrap();
    assert!(!saved.iter().find(|p| p.kind == "mail").unwrap().enabled);
    assert!(
        saved
            .iter()
            .find(|p| p.kind == "friendship")
            .unwrap()
            .enabled
    );
    assert!(notifications
        .preferences(other_id)
        .await
        .unwrap()
        .iter()
        .all(|p| p.enabled));
    let notice_page = text(
        call(
            app,
            "GET",
            "/notifications?notice=preferences_saved",
            recipient_cookie,
            "",
            &[],
        )
        .await,
    )
    .await;
    assert!(notice_page.contains("Настройки уведомлений сохранены."));
    assert!(notice_page.contains("name=\"pref_mail\" value=\"on\">"));
}
