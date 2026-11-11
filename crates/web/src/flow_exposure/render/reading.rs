//! Same-window page diagnostics from independent F/C aggregates, never attribution.
use crate::pages::escape_html as e;
use std::fmt::Write;
use tor_reading::services::audience::{ClosedBookEvidenceReport, ClosedChapterCounts};
use tor_recommendations::domain::{
    exposure::{Counts, ExposureBook, ObservationClass},
    exposure_health::ExposureHealth,
};

pub(super) fn intro(
    out: &mut String,
    days: i32,
    until: i64,
    reading: Option<&ClosedBookEvidenceReport>,
) {
    out.push_str(r#"<section class="book-section-card" data-testid="exposure-reading-scope"><h2>Показы и подтверждённые записи чтения</h2><p>Диагностика только книг на текущей странице за то же окно закрытых часов. Отметка «показы без подтверждённых стартов и завершений» требует подтверждённой работы зарегистрированных сборщиков, достаточной выборки показов и доступных записей чтения.</p><p>Считаются все сохранившиеся подтверждённые завершения глав в окне, включая следующую главу у прежнего читателя. Нуль означает отсутствие этих записей, а не отсутствие чтения вообще. Число первых пар «аккаунт–книга» для этой проверки не используется.</p><p>Анонимные показы не связываются с читателями. Это не конверсия и не причинный эффект продвижения. Общий итог и фильтры отчёта не пересчитываются по отметкам одной страницы.</p>"#);
    if let Some(source) =
        reading.filter(|source| source.days == days && source.until_epoch == until)
    {
        write!(
            out,
            "<p>Снимок источника чтения в UTC: {}.</p>",
            e(&source.measured_at_utc)
        )
        .expect("String");
    } else {
        out.push_str(r#"<p data-testid="exposure-reading-unavailable">Источник подтверждённых записей чтения недоступен для этого окна. Состояние неизвестно; нули не подставляются.</p>"#);
    }
    out.push_str("</section>");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Unknown,
    ObservedWithoutReading,
    ReadingRecorded,
}
impl State {
    fn key(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::ObservedWithoutReading => "observed-without-verified-reading",
            Self::ReadingRecorded => "reading-recorded",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Диагностика показов и записей чтения: неизвестно.",
            Self::ObservedWithoutReading => {
                "Наблюдаемые показы без подтверждённых стартов и завершений глав за это окно."
            }
            Self::ReadingRecorded => "В этом окне есть подтверждённые старты или завершения глав.",
        }
    }
}
fn classify(
    days: i32,
    until: i64,
    views: Counts,
    health: Option<&ExposureHealth>,
    reading: Option<ClosedChapterCounts>,
) -> State {
    let qualified = health
        .is_some_and(|health| health.until_epoch == until && health.current.complete(days))
        && views.classification() == ObservationClass::Observed;
    match reading {
        Some(counts) if qualified && counts.starts == 0 && counts.chapter_completions == 0 => {
            State::ObservedWithoutReading
        }
        Some(_) if qualified => State::ReadingRecorded,
        _ => State::Unknown,
    }
}
fn counts(
    days: i32,
    until: i64,
    book: i64,
    reading: Option<&ClosedBookEvidenceReport>,
) -> Option<ClosedChapterCounts> {
    reading
        .filter(|source| source.days == days && source.until_epoch == until)?
        .rows
        .iter()
        .find(|row| row.book_id == book)?
        .current
}
pub(super) fn row(
    out: &mut String,
    days: i32,
    until: i64,
    book: &ExposureBook,
    health: Option<&ExposureHealth>,
    reading: Option<&ClosedBookEvidenceReport>,
) {
    let current = counts(days, until, book.id, reading);
    let state = classify(days, until, book.counts, health, current);
    write!(out, r#"<section data-testid="exposure-reading" data-reading-state="{}"><h3>Подтверждённые записи чтения за то же окно</h3><p data-testid="exposure-reading-state">{}</p>"#,
        state.key(), state.label()).expect("String");
    let (starts, completions) = current.map_or_else(
        || ("неизвестно".into(), "неизвестно".into()),
        |counts| {
            (
                counts.starts.to_string(),
                counts.chapter_completions.to_string(),
            )
        },
    );
    write!(out, r#"<dl><dt>Подтверждённые старты глав</dt><dd data-metric="chapter-starts">{starts}</dd><dt>Все подтверждённые завершения глав</dt><dd data-metric="chapter-completions">{completions}</dd></dl>"#).expect("String");
    if state == State::Unknown {
        out.push_str("<p>Не выполнены условия полноты или выборки показов либо недоступно подтверждение записей чтения. Отметка об отсутствии записей не назначается.</p>");
    }
    out.push_str("</section>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_reading::services::audience::ClosedBookEvidenceRow;
    use tor_recommendations::domain::exposure_health::ExposureHealthPeriod;

    fn health() -> ExposureHealth {
        ExposureHealth {
            until_epoch: 3600,
            current: ExposureHealthPeriod {
                confirmed_hours: 24,
                ..Default::default()
            },
            previous: ExposureHealthPeriod::default(),
        }
    }
    fn observed() -> Counts {
        Counts {
            opportunities: 25,
            rendered: 20,
            views: 5,
            ..Default::default()
        }
    }
    #[test]
    fn badge_requires_current_health_sample_and_all_chapter_evidence() {
        let full = health();
        let zero = Some(ClosedChapterCounts::default());
        assert_eq!(
            classify(1, 3600, observed(), Some(&full), zero),
            State::ObservedWithoutReading
        );
        assert_eq!(
            classify(
                1,
                3600,
                observed(),
                Some(&full),
                Some(ClosedChapterCounts {
                    starts: 0,
                    chapter_completions: 1
                })
            ),
            State::ReadingRecorded,
            "an old account/book pair completing another chapter is positive reading evidence"
        );
        assert_eq!(
            classify(1, 3600, observed(), Some(&full), None),
            State::Unknown
        );
        assert_eq!(classify(1, 3600, observed(), None, zero), State::Unknown);
        assert_eq!(
            classify(1, 7200, observed(), Some(&full), zero),
            State::Unknown
        );
        for current in [
            ExposureHealthPeriod {
                confirmed_hours: 23,
                ..Default::default()
            },
            ExposureHealthPeriod {
                confirmed_hours: 24,
                loss_hours: 1,
                ..Default::default()
            },
            ExposureHealthPeriod {
                confirmed_hours: 24,
                error_hours: 1,
                ..Default::default()
            },
        ] {
            assert_eq!(
                classify(
                    1,
                    3600,
                    observed(),
                    Some(&ExposureHealth { current, ..full }),
                    zero
                ),
                State::Unknown
            );
        }
        for views in [
            Counts {
                rendered: 19,
                ..observed()
            },
            Counts {
                opportunities: 26,
                ..observed()
            },
            Counts {
                views: 4,
                ..observed()
            },
        ] {
            assert_eq!(classify(1, 3600, views, Some(&full), zero), State::Unknown);
        }
    }
    #[test]
    fn missing_ineligible_or_other_window_c_rows_never_become_zero() {
        let mut source = ClosedBookEvidenceReport {
            days: 1,
            until_epoch: 3600,
            measured_at_utc: "2026-11-11T01:01:00Z".into(),
            rows: vec![ClosedBookEvidenceRow {
                book_id: 10,
                current: None,
                previous: None,
            }],
        };
        assert_eq!(counts(1, 3600, 10, Some(&source)), None);
        source.rows[0].current = Some(ClosedChapterCounts::default());
        assert_eq!(
            counts(1, 3600, 10, Some(&source)),
            Some(ClosedChapterCounts::default())
        );
        assert_eq!(counts(1, 3600, 11, Some(&source)), None);
        assert_eq!(counts(7, 3600, 10, Some(&source)), None);
        assert_eq!(counts(1, 7200, 10, Some(&source)), None);
    }
}
