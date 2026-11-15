use super::SignalSources;
use tor_reading::services::audience::ClosedReadingReport;
use tor_recommendations::{
    domain::{
        exposure::{ExposureFilter, ExposureReport},
        exposure_health::ExposureHealth,
    },
    repositories::flow_signals::SignalObservation,
    services::flow_signals::{source_scope, Evidence, Rule, SignalWindow},
};
use tor_traffic_behavior::{
    domain::html_health::HtmlHealthReport, services::observed_audience::ObservedAudienceReport,
};

pub(super) fn unknown_window() -> SignalWindow {
    SignalWindow {
        until_epoch: None,
        label: "Источник недоступен; период не подтверждён".into(),
    }
}
fn closed_window(days: i32, until: i64) -> SignalWindow {
    let span = i64::from(days) * 86_400;
    SignalWindow {
        until_epoch: Some(until),
        label: format!(
            "{} — {}; ранее {} — {}",
            crate::flow_runtime::utc(until - span),
            crate::flow_runtime::utc(until),
            crate::flow_runtime::utc(until - 2 * span),
            crate::flow_runtime::utc(until - span)
        ),
    }
}
fn observation(
    rule: Rule,
    scope: &str,
    window: &SignalWindow,
    evidence: Evidence,
) -> SignalObservation {
    SignalObservation {
        rule,
        scope: scope.into(),
        window: window.clone(),
        evidence,
    }
}

pub(super) fn html(
    days: i32,
    report: Option<(ObservedAudienceReport, Option<HtmlHealthReport>)>,
) -> Vec<SignalObservation> {
    let scope = source_scope(&["html", "closed-utc-hours", "anonymous-v2"]);
    let mut evidence = Evidence::missing(true);
    let mut window = unknown_window();
    if let Some((report, health)) = report {
        evidence.available = true;
        evidence.expected_units = days * 24;
        evidence.current = report.current.saved_total;
        evidence.previous = report.previous.saved_total;
        window = closed_window(days, report.current.until_epoch);
        if let Some(health) =
            health.filter(|health| health.until_epoch == report.current.until_epoch)
        {
            evidence.current_units = health.current.confirmed_hours;
            evidence.previous_units = health.previous.confirmed_hours;
            evidence.known_discards =
                i64::from(health.current.loss_hours) + i64::from(health.previous.loss_hours);
            evidence.source_failed =
                health.current.error_hours > 0 || health.previous.error_hours > 0;
            if health.current.confirmed_hours == days * 24 && !health.current.has_failure() {
                evidence.current = Some(evidence.current.unwrap_or(0));
            }
            if health.previous.confirmed_hours == days * 24 && !health.previous.has_failure() {
                evidence.previous = Some(evidence.previous.unwrap_or(0));
            }
        }
    }
    [Rule::HtmlQuality, Rule::HtmlChange]
        .into_iter()
        .map(|rule| observation(rule, &scope, &window, evidence))
        .collect()
}
pub(super) fn reading(days: i32, report: Option<ClosedReadingReport>) -> Vec<SignalObservation> {
    let scope = source_scope(&["canonical-reading", "first-verified-start-and-book-pair-v1"]);
    let mut starts = Evidence::missing(true);
    let mut completions = starts;
    let mut window = unknown_window();
    if let Some(report) = report {
        window = closed_window(days, report.until_epoch);
        starts.available = true;
        starts.current_units = days * 24;
        starts.previous_units = days * 24;
        starts.expected_units = days * 24;
        starts.current = Some(report.current.starts);
        starts.previous = Some(report.previous.starts);
        completions = starts;
        completions.current = Some(report.current.completed_book_pairs);
        completions.previous = Some(report.previous.completed_book_pairs);
    }
    vec![
        observation(Rule::ReadingStarts, &scope, &window, starts),
        observation(Rule::ReadingCompletions, &scope, &window, completions),
    ]
}
pub(super) fn exposure_filter(days: i32) -> ExposureFilter {
    ExposureFilter {
        days,
        surface: None,
        after: 0,
        class: None,
        group: None,
        campaign: None,
        editorial: None,
    }
}
fn ack(rendered: i64, opportunities: i64) -> Option<i32> {
    (opportunities > 0)
        .then(|| (i128::from(rendered) * 1000 / i128::from(opportunities)).clamp(0, 1000) as i32)
}
pub(super) fn exposure(
    days: i32,
    sources: &SignalSources,
    report: Option<(ExposureReport, ExposureHealth)>,
) -> Vec<SignalObservation> {
    let scope = source_scope(&[
        "cards",
        "closed-utc-hours-ttl900",
        sources.origin.as_deref().unwrap_or("unconfigured"),
    ]);
    let mut quality = Evidence::missing(sources.observations_enabled);
    let mut books = quality;
    let mut genres = quality;
    let mut window = unknown_window();
    if let Some((report, health)) =
        report.filter(|(report, health)| report.until_epoch == health.until_epoch)
    {
        window = closed_window(days, report.until_epoch);
        quality.available = true;
        quality.expected_units = days * 24;
        quality.current_units = health.current.confirmed_hours;
        quality.previous_units = health.previous.confirmed_hours;
        quality.known_discards =
            i64::from(health.current.loss_hours) + i64::from(health.previous.loss_hours);
        quality.source_failed = health.current.error_hours > 0 || health.previous.error_hours > 0;
        quality.current = report.current.known_views();
        quality.previous = report.previous.known_views();
        quality.current_ack_per_mille = ack(report.current.rendered, report.current.opportunities);
        quality.previous_ack_per_mille =
            ack(report.previous.rendered, report.previous.opportunities);
        quality.current_sample = report.current.rendered;
        quality.previous_sample = report.previous.rendered;
        quality.current_opportunities = report.current.opportunities;
        quality.previous_opportunities = report.previous.opportunities;
        books = quality;
        books.current = Some(report.low_observed_books);
        books.previous = None;
        books.known_discards = i64::from(health.current.loss_hours);
        books.source_failed = health.current.error_hours > 0;
        books.unknown_entities = report.unknown_books;
        genres = books;
        genres.current = Some(report.genres.low_observed);
        genres.unknown_entities = report.genres.unknown;
    }
    vec![
        observation(Rule::ExposureQuality, &scope, &window, quality),
        observation(Rule::ExposureChange, &scope, &window, quality),
        observation(Rule::UnderexposedBooks, &scope, &window, books),
        observation(Rule::UnderexposedGenres, &scope, &window, genres),
    ]
}
