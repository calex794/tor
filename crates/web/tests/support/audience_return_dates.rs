//! Calendar assertions for FLOW reports that render an explicit UTC measurement time.

use chrono::{DateTime, Duration as CalendarDuration};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub async fn wait_for_fixture_day() {
    // These fixtures seed cohorts in several separate SQL commands. Give
    // them time to finish on one UTC day before checking their exact counts.
    const DAY_SECONDS: u64 = 86_400;
    const FIXTURE_WINDOW_SECONDS: u64 = 30;
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let seconds_left = DAY_SECONDS - elapsed.as_secs() % DAY_SECONDS;
    if seconds_left <= FIXTURE_WINDOW_SECONDS {
        tokio::time::sleep(Duration::from_secs(seconds_left)).await;
    }
}

pub fn assert_period_bounds(html: &str, current: &str, previous: &str, days: i32) {
    let timestamp = html
        .split_once("<p>Время расчёта: <time datetime=\"")
        .and_then(|(_, tail)| tail.split_once('"'))
        .map(|(timestamp, _)| timestamp)
        .expect("report measurement timestamp");
    assert!(timestamp.ends_with('Z'), "report measurement is UTC");
    let today = DateTime::parse_from_rfc3339(timestamp)
        .unwrap()
        .date_naive();
    let days = i64::from(days);
    for (name, section, ages) in [
        ("current", current, [days - 1, -1]),
        ("previous", previous, [2 * days - 1, days - 1]),
    ] {
        for age in ages {
            let date = today - CalendarDuration::days(age);
            assert!(
                section.contains(&format!("datetime=\"{date}T00:00:00Z\"")),
                "{name}: {date} UTC boundary for report measured at {timestamp}"
            );
        }
    }
}

#[test]
fn report_periods_use_the_measurement_day_at_utc_rollover() {
    let before = r#"<p>Время расчёта: <time datetime="2026-11-18T23:59:59.999999Z">"#;
    let after = r#"<p>Время расчёта: <time datetime="2026-11-19T00:00:00.000000Z">"#;
    let nov_17_to_18 =
        r#"<time datetime="2026-11-17T00:00:00Z"><time datetime="2026-11-18T00:00:00Z">"#;
    let nov_18_to_19 =
        r#"<time datetime="2026-11-18T00:00:00Z"><time datetime="2026-11-19T00:00:00Z">"#;
    let nov_19_to_20 =
        r#"<time datetime="2026-11-19T00:00:00Z"><time datetime="2026-11-20T00:00:00Z">"#;
    assert_period_bounds(before, nov_18_to_19, nov_17_to_18, 1);
    assert_period_bounds(after, nov_19_to_20, nov_18_to_19, 1);
}
