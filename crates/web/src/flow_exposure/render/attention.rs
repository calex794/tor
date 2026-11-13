//! Read-only diagnostic quality for anonymous placement evidence, never CTR or ROI.
use super::{known, url, ReportWindow};
use crate::pages::escape_html as e;
use std::fmt::Write;
use tor_contracts::exposure::ExposureSurface;
use tor_recommendations::domain::exposure::{
    Counts, ExposureFilter, ExposurePlacement, ExposureReport, ObservationClass,
};
use tor_recommendations::domain::exposure_health::{ExposureHealth, ExposureHealthPeriod};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Signal {
    Unknown,
    Low,
    Observed,
}
impl Signal {
    fn key(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Low => "low",
            Self::Observed => "observed",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "неизвестно: неполный сбор или малая выборка",
            Self::Low => "достаточный сбор, менее пяти подтверждённых показов",
            Self::Observed => "достаточный сбор для операционной диагностики",
        }
    }
}
fn quality(c: Counts, period: Option<ExposureHealthPeriod>, days: i32) -> Signal {
    if !period.is_some_and(|p| p.complete(days))
        || c.opportunities <= 0
        || c.rendered < 0
        || c.rendered > c.opportunities
        || c.views < 0
        || c.views > c.rendered
        || c.clicks < 0
        || c.clicks > c.rendered
    {
        return Signal::Unknown;
    }
    match c.classification() {
        ObservationClass::Unknown => Signal::Unknown,
        ObservationClass::LowObserved => Signal::Low,
        ObservationClass::Observed => Signal::Observed,
    }
}
fn change(current: Counts, previous: Counts, now: Signal, before: Signal) -> Option<i128> {
    (now != Signal::Unknown && before != Signal::Unknown)
        .then(|| i128::from(current.views) - i128::from(previous.views))
}
fn coverage(c: Counts) -> String {
    if c.opportunities <= 0 || c.rendered < 0 || c.rendered > c.opportunities {
        return "неизвестно".into();
    }
    format!(
        "{}%",
        i128::from(c.rendered) * 100 / i128::from(c.opportunities)
    )
}
fn row(
    out: &mut String,
    q: &ExposureFilter,
    window: ReportWindow,
    item: &ExposurePlacement,
    health: Option<&ExposureHealth>,
) {
    let label = match item.surface {
        ExposureSurface::Home => "Главная",
        ExposureSurface::Catalog => "Каталог",
        ExposureSurface::Search => "Поиск",
        ExposureSurface::BookDetail => "Карточка книги",
        ExposureSurface::ReaderEnd => "После бесплатной главы",
    };
    let source = if item.editorial {
        "Редакционное продвижение"
    } else {
        "Органические рекомендации"
    };
    let mut drill = q.clone();
    drill.surface = Some(item.surface);
    drill.editorial = Some(item.editorial);
    drill.after = 0;
    drill.class = None;
    let views = known(item.current.known_views());
    let clicks = if item.current.rendered > 0 {
        item.current.clicks.to_string()
    } else {
        "неизвестно".to_owned()
    };
    let previous = known(item.previous.known_views());
    let cover = coverage(item.current);
    let current_quality = quality(item.current, health.map(|h| h.current), q.days);
    let previous_quality = quality(item.previous, health.map(|h| h.previous), q.days);
    let delta = change(item.current, item.previous, current_quality, previous_quality);
    let delta_label = delta.map_or_else(|| "неизвестно".into(), |n| format!("{n:+}"));
    let delta_state = if delta.is_some() { "known" } else { "unknown" };
    write!(
        out,
        r#"<li data-attn-surface="{}" data-attn-source="{}" data-attn-quality="{}" data-attn-delta="{}"><a href="{}">{label}</a> — {source}: карточек {}, DOM {}, показов {views}, кликов {clicks}; в предыдущем окне показов {previous}. DOM среди зарегистрированных: {cover}. Качество измерения: {}. Изменение подтверждённых показов: {delta_label}.</li>"#,
        item.surface.as_str(),
        if item.editorial { "editorial" } else { "organic" },
        current_quality.key(),
        delta_state,
        e(&url(&drill, window, 0)),
        item.current.opportunities,
        item.current.rendered,
        current_quality.label(),
    )
    .expect("String");
}
pub(super) fn placements(
    out: &mut String,
    q: &ExposureFilter,
    window: ReportWindow,
    report: &ExposureReport,
    health: Option<&ExposureHealth>,
) {
    // Never compare calendar windows, different UTC anchors, or incomplete
    // collector hours: unknown is not an invented zero or a decline.
    let health = if window == ReportWindow::Closed {
        health.filter(|h| h.until_epoch == report.until_epoch)
    } else {
        None
    };
    out.push_str(r#"<section class="book-section-card" data-testid="attention-placements"><h2>ВНИМАНИЕ: источник × место показа</h2><p>Сравнение показов доступно только для двух полных UTC-окон с подтверждением всех часов сборщиками без потерь и ошибок. В каждом окне нужно не менее 20 подтверждений DOM и не менее 80% зарегистрированных карточек. Без этого изменение неизвестно. Процент DOM относится только к зарегистрированным карточкам, а не ко всему трафику.</p><p>Клики не являются конверсией из показов. Подтверждённое чтение и покупки нельзя приписывать отдельным карточкам.</p>"#);
    if report.placements.is_empty() {
        out.push_str("<p>Зарегистрированных карточек по выбранным фильтрам нет; охват всего трафика неизвестен.</p>");
    } else {
        out.push_str(r#"<ul class="audience-attention-list">"#);
        for placement in &report.placements {
            row(out, q, window, placement, health);
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>");
}
#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> ExposureFilter {
        ExposureFilter {
            days: 1,
            surface: None,
            after: 0,
            class: None,
            group: None,
            campaign: None,
            editorial: None,
        }
    }
    fn sample(views: i64) -> Counts {
        Counts {
            opportunities: 25,
            rendered: 20,
            views,
            clicks: 2,
            ..Counts::default()
        }
    }
    fn healthy() -> ExposureHealth {
        ExposureHealth {
            until_epoch: 3600,
            current: ExposureHealthPeriod {
                confirmed_hours: 24,
                ..Default::default()
            },
            previous: ExposureHealthPeriod {
                confirmed_hours: 24,
                ..Default::default()
            },
        }
    }
    #[test]
    fn missing_dom_is_unknown_not_zero_or_a_trend() {
        let mut html = String::new();
        row(
            &mut html,
            &filter(),
            ReportWindow::Closed,
            &ExposurePlacement {
                surface: ExposureSurface::ReaderEnd,
                editorial: true,
                current: Counts {
                    opportunities: 14,
                    ..Counts::default()
                },
                previous: Counts::default(),
            },
            None,
        );
        assert!(html.contains(r#"data-attn-surface="reader_end""#));
        assert!(html.contains(r#"data-attn-source="editorial""#));
        assert!(html.contains(r#"data-attn-quality="unknown" data-attn-delta="unknown""#));
        assert!(html.contains("days=1&amp;window=closed&amp;surface=reader_end&amp;source=editorial"));
        assert!(html.contains("карточек 14, DOM 0, показов неизвестно, кликов неизвестно"));
        assert!(html.contains("Изменение подтверждённых показов: неизвестно"));
        assert!(!html.contains("конверсия"));
    }
    #[test]
    fn complete_matching_windows_allow_only_descriptive_signed_change() {
        let full = healthy();
        let now = quality(sample(9), Some(full.current), 1);
        let before = quality(sample(5), Some(full.previous), 1);
        assert_eq!(now, Signal::Observed);
        assert_eq!(before, Signal::Observed);
        assert_eq!(change(sample(9), sample(5), now, before), Some(4));
        let mut html = String::new();
        row(
            &mut html,
            &filter(),
            ReportWindow::Closed,
            &ExposurePlacement {
                surface: ExposureSurface::Home,
                editorial: false,
                current: sample(9),
                previous: sample(5),
            },
            Some(&full),
        );
        assert!(html.contains(r#"data-attn-quality="observed" data-attn-delta="known""#));
        assert!(html.contains("DOM среди зарегистрированных: 80%"));
        assert!(html.contains("Изменение подтверждённых показов: +4"));
    }
    #[test]
    fn incomplete_health_invalid_sample_and_stale_anchor_are_unknown() {
        let full = healthy();
        assert_eq!(quality(sample(9), None, 1), Signal::Unknown);
        assert_eq!(quality(sample(9), Some(full.current), 7), Signal::Unknown);
        assert_eq!(
            quality(
                sample(9),
                Some(ExposureHealthPeriod {
                    loss_hours: 1,
                    ..full.current
                }),
                1
            ),
            Signal::Unknown
        );
        assert_eq!(
            quality(Counts { rendered: 19, ..sample(9) }, Some(full.current), 1),
            Signal::Unknown
        );
        assert_eq!(
            quality(Counts { rendered: 26, ..sample(9) }, Some(full.current), 1),
            Signal::Unknown
        );
        assert_eq!(coverage(Counts::default()), "неизвестно");
        let a = quality(sample(9), Some(full.current), 1);
        assert_eq!(change(sample(9), sample(5), a, Signal::Unknown), None);
        let report = ExposureReport {
            checked_at_epoch: 3600,
            until_epoch: 7200,
            books: 0,
            current: Counts::default(),
            previous: Counts::default(),
            low_observed_books: 0,
            unknown_books: 0,
            genres: Default::default(),
            rows: Vec::new(),
            next: None,
            groups: Vec::new(),
            placements: vec![ExposurePlacement {
                surface: ExposureSurface::Home,
                editorial: false,
                current: sample(9),
                previous: sample(5),
            }],
        };
        let mut html = String::new();
        placements(&mut html, &filter(), ReportWindow::Closed, &report, Some(&full));
        assert!(html.contains(r#"data-attn-quality="unknown" data-attn-delta="unknown""#));
        html.clear();
        placements(&mut html, &filter(), ReportWindow::Calendar, &report, Some(&full));
        assert!(html.contains(r#"data-attn-quality="unknown" data-attn-delta="unknown""#));
    }
}
