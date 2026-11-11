//! Authorized exact-ID inspection; the HTTP process never opens SQLite.
use axum::http::StatusCode;
use std::{
    fmt::Write,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Semaphore;
use tor_observability::services::{
    lookup::{self, LookupResult},
    trace::background::lookup::{self as child_lookup, BackgroundPage},
};

static READERS: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub(super) struct Inspection {
    root: Result<LookupResult, ()>,
    children: Result<BackgroundPage, ()>,
}

/// The permit lives inside the blocking task even when the HTTP waiter times
/// out. At most four stalled socket lookups can consume blocking workers.
pub(super) async fn inspect(socket: PathBuf, parent: u128) -> Option<Inspection> {
    let permit = READERS
        .get_or_init(|| Arc::new(Semaphore::new(4)))
        .clone()
        .try_acquire_owned()
        .ok()?;
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        Inspection {
            root: lookup::request(&socket, parent).map_err(|_| ()),
            children: child_lookup::request(&socket, parent).map_err(|_| ()),
        }
    });
    tokio::time::timeout(Duration::from_secs(4), task)
        .await
        .ok()?
        .ok()
}

impl Inspection {
    pub(super) fn status(&self) -> StatusCode {
        let found_root = self.root.as_ref().is_ok_and(|r| r.found.is_some());
        let found_children = self.children.as_ref().is_ok_and(|p| p.matches > 0);
        if found_root || found_children {
            StatusCode::OK
        } else if self.root.is_ok() && self.children.is_ok() {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }

    pub(super) fn details(&self) -> String {
        let mut details = match &self.root {
            Ok(root) => super::trace_details(root),
            Err(()) => "<section class=\"card\" role=\"status\"><h2>HTTP-трасса недоступна</h2><p>Состояние корневой записи неизвестно. Доступные фоновые попытки показаны отдельно.</p></section>".to_owned(),
        };
        details.push_str(&children_html(self.children.as_ref().ok()));
        details
    }
}

fn children_html(page: Option<&BackgroundPage>) -> String {
    let mut html = "<section class=\"card\" aria-labelledby=\"background-title\"><h2 id=\"background-title\">Фоновый импорт глав</h2><p>Показаны отдельно выполнявшиеся попытки импорта с этим исходным HTTP-запросом. Их время не прибавляется к HTTP. Остальные фоновые процессы здесь не измеряются.</p>".to_owned();
    let Some(page) = page else {
        html.push_str("<p role=\"status\">Архив фоновых попыток недоступен или его версия не поддерживает этот запрос. Число попыток неизвестно.</p></section>");
        return html;
    };
    write!(
        html,
        "<p data-background-matches=\"{}\">Сохранено попыток: {}. Показано: {}.</p>",
        page.matches,
        page.matches,
        page.items.len()
    )
    .expect("write String");
    if page.items.is_empty() {
        html.push_str("<p>Сохранённых попыток не найдено. Это не доказывает отсутствие работы или её успешное завершение: задача могла быть старой, ещё не начаться, запись могла истечь или потеряться при аварии.</p>");
    }
    for (at, trace) in &page.items {
        write!(
            html,
            "<article class=\"card\" data-background-outcome=\"{}\"><h3>Попытка {}</h3><p><code style=\"overflow-wrap:anywhere\">{:032x}</code></p><dl><dt>Результат</dt><dd>{}</dd><dt>Последняя начатая фаза</dt><dd>{}</dd><dt>Сохранена (Unix UTC)</dt><dd>{at}</dd><dt>Время попытки</dt><dd>{:.2} мс</dd><dt>Получение источника</dt><dd>{:.2} мс</dd><dt>Разбор текста</dt><dd>{:.2} мс</dd><dt>Операции очереди и запись глав</dt><dd>{:.2} мс</dd><dt>Попытка очистки</dt><dd>{:.2} мс</dd></dl></article>",
            trace.outcome.label(),
            trace.attempt,
            trace.trace_id,
            trace.outcome.label(),
            trace.phase,
            trace.duration_us as f64 / 1000.0,
            trace.phase_us[0] as f64 / 1000.0,
            trace.phase_us[1] as f64 / 1000.0,
            trace.phase_us[2] as f64 / 1000.0,
            trace.phase_us[3] as f64 / 1000.0
        )
        .expect("write String");
    }
    html.push_str("<p>Фазы: 1 — источник, 2 — разбор, 3 — запись, 4 — очистка. Это время ожидания и выполнения этапов, а не время SQL внутри PostgreSQL или загрузка CPU. Completed подтверждает результат импорта, но не удаление временного объекта. Interrupted означает прерывание наблюдения; состояние задачи проверяется отдельно. Аварийное завершение процесса может не оставить запись.</p></section>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_observability::services::trace::background::{BackgroundTrace, Outcome};

    fn children() -> BackgroundPage {
        BackgroundPage {
            matches: 1,
            items: vec![(
                100,
                BackgroundTrace {
                    parent_trace_id: 16,
                    trace_id: 32,
                    attempt: 2,
                    outcome: Outcome::Interrupted,
                    phase: 2,
                    duration_us: 10,
                    phase_us: [1, 2, 0, 0],
                },
            )],
        }
    }

    #[test]
    fn unavailable_is_unknown_and_empty_is_not_completion() {
        let unavailable = children_html(None);
        assert!(unavailable.contains("неизвестно"));
        assert!(!unavailable.contains("data-background-matches"));
        let empty = children_html(Some(&BackgroundPage::default()));
        assert!(empty.contains("data-background-matches=\"0\""));
        assert!(empty.contains("не доказывает"));
    }

    #[test]
    fn missing_root_does_not_hide_existing_children() {
        let report = Inspection {
            root: Err(()),
            children: Ok(children()),
        };
        assert_eq!(report.status(), StatusCode::OK);
        assert!(report.details().contains("HTTP-трасса недоступна"));
        assert!(report
            .details()
            .contains("data-background-outcome=\"interrupted\""));
        assert!(report.details().contains("не прибавляется"));
    }

    #[test]
    fn complete_empty_archive_and_unknown_archive_are_distinct() {
        let root = LookupResult {
            archived: 0,
            matches: 0,
            found: None,
        };
        let mut report = Inspection {
            root: Ok(root),
            children: Ok(BackgroundPage::default()),
        };
        assert_eq!(report.status(), StatusCode::NOT_FOUND);
        report.children = Err(());
        assert_eq!(report.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
