#[path = "reading_time_fixture.rs"]
mod fixture;
use axum::http::StatusCode;
use fixture::*;
use std::time::{Duration, Instant};
use tor_accounts::services::security::session_token_digest;

const PATH: &str = "/admin/audience?view=reading_hours&days=1&time_zone=Europe%2FAmsterdam";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn reading_clock_native_periods_zones_dst_unknown_and_current_authority() {
    let f = Fixture::new().await;
    f.get("/admin/audience?view=invalid", "", StatusCode::UNAUTHORIZED)
        .await;
    f.get(PATH, &f.cookie, StatusCode::FORBIDDEN).await;
    f.role(true);
    f.seed();
    let before = f.fingerprint();
    for days in [1, 7, 30, 90] {
        let html = f
            .get(
                &format!("/admin/audience?view=reading_hours&days={days}&time_zone=UTC"),
                &f.cookie,
                StatusCode::OK,
            )
            .await;
        let current = section(&html, "data-testid=\"reading-time-current\"");
        let previous = section(&html, "data-testid=\"reading-time-previous\"");
        assert_eq!(
            metric(current, "chapter-starts"),
            if days == 1 { "2" } else { "3" }
        );
        assert_eq!(
            metric(current, "chapter-completions"),
            if days == 1 { "2" } else { "3" }
        );
        assert_eq!(
            metric(previous, "chapter-starts"),
            if days == 1 { "1" } else { "0" }
        );
        assert_eq!(metric(current, "utc-hours"), (days * 24).to_string());
        assert_eq!(metric(previous, "utc-hours"), (days * 24).to_string());
        assert_eq!(
            html.matches("data-reading-epoch=").count(),
            2 * (days as usize * 24).min(168)
        );
        assert_eq!(html.matches("data-reading-hour=").count(), 24);
        assert_eq!(html.matches("data-reading-weekday=").count(), 7);
        assert!(!html.contains("audience-observed-current"));
        assert!(html.contains("Повторное чтение уже учтённой главы здесь не измеряется"));
        assert!(!html.contains("example.invalid"));
        assert!(!html.contains("Clock-only fixture"));
        assert!(!html.contains("book_id"));
    }
    let html = f.get(PATH, &f.cookie, StatusCode::OK).await;
    let rows = html
        .split("data-testid=\"reading-time-current-hours\"")
        .nth(1)
        .unwrap()
        .split("</table>")
        .next()
        .unwrap();
    assert_eq!(rows.matches("2026-10-25 02:00").count(), 2);
    assert!(rows.contains("data-offset-minutes=\"120\""));
    assert!(rows.contains("data-offset-minutes=\"60\""));
    let profile = html
        .split("data-testid=\"reading-time-hour-profile\"")
        .nth(1)
        .unwrap();
    let hour = profile
        .split("data-reading-hour=\"2\"")
        .nth(1)
        .unwrap()
        .split("</tr>")
        .next()
        .unwrap();
    assert!(hour.contains("data-period=\"current\" data-utc-hours=\"2\""));
    assert_eq!(metric(hour, "chapter-completions"), "2");
    for zone in ["Europe%2FMoscow", "Asia%2FYekaterinburg"] {
        let html = f
            .get(
                &format!("/admin/audience?view=reading_hours&days=1&time_zone={zone}"),
                &f.cookie,
                StatusCode::OK,
            )
            .await;
        assert_eq!(
            metric(
                section(&html, "data-testid=\"reading-time-current\""),
                "chapter-starts"
            ),
            "2"
        );
    }
    for query in [
        "view=invalid",
        "view=",
        "view=reading_hours&view=overview",
        "view=reading_hours&days=2",
        "view=reading_hours&time_zone=Asia%2FKolkata",
        "view=reading_hours&time_zone=Asia%2FKathmandu",
        "view=reading_hours&time_zone=%3Cscript%3E",
    ] {
        f.get(
            &format!("/admin/audience?{query}"),
            &f.cookie,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
    assert_eq!(before, f.fingerprint(), "GET must not mutate source facts");
    f.clock("2026-03-30T00:37:00Z");
    let spring = f.get(PATH, &f.cookie, StatusCode::OK).await;
    let hour = spring
        .split("data-reading-hour=\"2\"")
        .nth(1)
        .unwrap()
        .split("</tr>")
        .next()
        .unwrap();
    assert!(hour.contains("data-period=\"current\" data-utc-hours=\"0\""));
    assert!(hour.contains("Не входит в выбранный период"));
    f.clock(FALL);
    f.scope("DELETE FROM outbox_events WHERE id=(SELECT source_event_id FROM reader_chapter_completions WHERE chapter_id=7002)");
    let unknown = f.get(PATH, &f.cookie, StatusCode::OK).await;
    assert_eq!(
        metric(
            section(&unknown, "data-testid=\"reading-time-current\""),
            "chapter-starts"
        ),
        "Неизвестно"
    );
    assert_eq!(
        metric(
            section(&unknown, "data-testid=\"reading-time-previous\""),
            "chapter-completions"
        ),
        "1"
    );
    assert!(unknown.contains("reading-time-comparison-unknown"));
    f.proof(7002, true);
    // The time view must not depend on an overview-only H source.
    f.scope(&format!(
        "CREATE TABLE {}.ril_flow_observed_hourly (LIKE public.ril_flow_observed_hourly INCLUDING ALL);REVOKE ALL ON {}.ril_flow_observed_hourly FROM tor_app;",
        f.schema, f.schema
    ));
    f.get(PATH, &f.cookie, StatusCode::OK).await;
    let overview = f
        .get(
            "/admin/audience?view=overview&days=1",
            &f.cookie,
            StatusCode::SERVICE_UNAVAILABLE,
        )
        .await;
    assert!(overview.contains("audience-observed-unavailable"));
    let lock = f.lock();
    let started = Instant::now();
    let failed = f
        .get(PATH, &f.cookie, StatusCode::SERVICE_UNAVAILABLE)
        .await;
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(failed.contains("reading-time-unavailable"));
    assert!(!failed.contains("data-metric="));
    tokio::time::timeout(
        Duration::from_millis(300),
        f.accounts.session_actor(&session_token_digest(&f.token)),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    drop(lock);
    f.get(PATH, &f.cookie, StatusCode::OK).await;
    f.role(false);
    f.get(PATH, &f.cookie, StatusCode::FORBIDDEN).await;
    f.role(true);
    f.accounts
        .revoke_session(&session_token_digest(&f.token))
        .await
        .unwrap();
    f.get(PATH, &f.cookie, StatusCode::UNAUTHORIZED).await;
    f.catalog.close_for_test().await;
}
