use super::{evidence::unknown_window, SignalSources};
use tor_recommendations::{
    repositories::flow_signals::SignalObservation,
    services::flow_signals::{source_scope, Evidence, Rule, SignalWindow},
};
use tor_search::domain::{
    public_pages::{PageState, PublicPageReport},
    seo_analytics::{Provider, SeoReport},
};

pub(super) fn seo(
    days: i32,
    provider: Provider,
    sources: &SignalSources,
    report: Option<SeoReport>,
) -> Vec<SignalObservation> {
    let (quality_rule, clicks_rule, property) = match provider {
        Provider::Google => (
            Rule::GoogleQuality,
            Rule::GoogleClicks,
            sources.google_property.as_deref(),
        ),
        Provider::Yandex => (
            Rule::YandexQuality,
            Rule::YandexClicks,
            sources.yandex_property.as_deref(),
        ),
    };
    let property = property.unwrap_or("unconfigured");
    let origin = sources.seo_origin.as_deref().unwrap_or("unconfigured");
    let scope = source_scope(&["search-provider", provider.key(), property, origin]);
    let mut quality = Evidence::missing(sources.seo_enabled);
    let mut window = unknown_window();
    if let Some(report) = report.filter(|report| report.provider == provider) {
        window = SignalWindow {
            until_epoch: None,
            label: format!(
                "{}: {} — {}; ранее {} — {}",
                provider.calendar(),
                report.current.from,
                report.current.until,
                report.previous.from,
                report.previous.until
            ),
        };
        if let Some(source) = report.sources.iter().find(|source| {
            source.source.key.provider == provider
                && source.source.key.property == property
                && source.source.key.origin == origin
        }) {
            quality.available = true;
            quality.expected_units = days;
            quality.current_units = report.current.total_days.try_into().unwrap_or(i32::MAX);
            quality.previous_units = report.previous.total_days.try_into().unwrap_or(i32::MAX);
            quality.current = report.current.clicks;
            quality.previous = report.previous.clicks;
            quality.source_failed = source.last_error.is_some();
            quality.limited_units = (report.current.limited_days + report.previous.limited_days)
                .try_into()
                .unwrap_or(i32::MAX);
            quality.omitted_rows = report
                .current
                .omitted_rows
                .saturating_add(report.previous.omitted_rows);
        }
    }
    let mut clicks = quality;
    // Provider totals are independent of the bounded URL detail inventory.
    clicks.limited_units = 0;
    clicks.omitted_rows = 0;
    vec![
        SignalObservation {
            rule: quality_rule,
            scope: scope.clone(),
            window: window.clone(),
            evidence: quality,
        },
        SignalObservation {
            rule: clicks_rule,
            scope,
            window,
            evidence: clicks,
        },
    ]
}

pub(super) fn pages(origin: Option<&str>, report: Option<PublicPageReport>) -> SignalObservation {
    let scope = source_scope(&["public-page-head-sample", origin.unwrap_or("unconfigured")]);
    let mut evidence = Evidence::missing(true);
    let mut window = unknown_window();
    if let Some(report) = report {
        window = SignalWindow {
            until_epoch: None,
            label: report
                .checked_at
                .clone()
                .map(|at| format!("Последняя выборка: {at}"))
                .unwrap_or_else(|| "Подтверждённой проверки ещё нет".into()),
        };
        evidence.available = true;
        evidence.current = Some(report.lost_targets());
        evidence.current_units = report.checked_targets();
        evidence.expected_units = report.expected_targets();
        evidence.unknown_entities = report
            .rows
            .iter()
            .filter(|row| row.state == PageState::Unknown)
            .count() as i64;
    }
    SignalObservation {
        rule: Rule::PublicPageLoss,
        scope,
        window,
        evidence,
    }
}
