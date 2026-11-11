use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_recommendations::{
    repositories::flow_signals::{SignalReport, SignalState},
    services::flow_signals::{EventKind, Reason, Rule, Status},
};

pub(super) fn page(days: i32, report: Option<&SignalReport>) -> Html<String> {
    let mut body = format!(
        r#"<section class="section"><div class="container author-width"><p class="eyebrow">Администратору сайта</p><h1>Сигналы аудитории</h1>{}<p>Сохранённые результаты проверок общих чисел и полноты измерения. Открытие страницы не запускает расчёт и не меняет журнал.</p><form class="author-form" method="get" action="/admin/audience/signals"><label for="signal-days">Длительность периода</label><select id="signal-days" name="days">"#,
        crate::flow_navigation::links(days)
    );
    for value in [1, 7, 30, 90] {
        let selected = if days == value { " selected" } else { "" };
        write!(
            body,
            r#"<option value="{value}"{selected}>{value} дн.</option>"#
        )
        .expect("String");
    }
    body.push_str(
        r#"</select><p><button class="button" type="submit">Показать</button></p></form>"#,
    );
    if report.is_none() {
        body.push_str(r#"<p role="alert">Журнал временно недоступен. Состояние неизвестно; нулевые значения не подставляются.</p>"#);
    }
    for rule in Rule::ALL {
        let saved = report.and_then(|report| report.states.iter().find(|state| state.rule == rule));
        let status = saved
            .zip(report)
            .map(|(state, report)| state.effective_result(report.checked_at_ms).status)
            .unwrap_or(Status::Unknown);
        write!(body, r#"<section class="book-section-card" data-testid="signal-state" data-rule="{}" data-state="{}"><h2>{}</h2><p><strong>{}</strong></p>"#,
            rule.code(), status.code(), title(rule), status_label(status)).expect("String");
        if let Some(state) = saved {
            if state.stale(report.expect("saved report").checked_at_ms) {
                body.push_str("<p>Результат старше пяти минут или его время некорректно. Это не подтверждение восстановления.</p>");
            } else {
                write!(body, "<p>{}</p>", reason(state.result.reason)).expect("String");
            }
            if state.episode.reason.is_some() {
                body.push_str(r#"<p data-testid="signal-open-episode">Ранее открытый эпизод ещё не закрыт подтверждённым восстановлением.</p>"#);
            }
            if rule == Rule::PublicPageLoss && status == Status::Healthy {
                body.push_str("<p>В текущей проверяемой выборке нет подтверждённых потерь. Восстановление ранее проверенных URL не утверждается.</p>");
            }
            evidence(&mut body, state);
        } else {
            body.push_str("<p>Подтверждённой проверки этого источника ещё нет.</p>");
        }
        write!(
            body,
            r#"<p><a href="{}">Открыть данные источника</a></p></section>"#,
            source_link(rule, days)
        )
        .expect("String");
    }
    body.push_str(r#"<section class="book-section-card"><h2>Журнал изменений</h2><p>Каждая запись хранит состояние и границы периода в момент проверки.</p>"#);
    if let Some(report) = report {
        if report.events.is_empty() {
            body.push_str("<p>В выбранной части журнала нет записей.</p>");
        }
        for event in &report.events {
            let label = match (event.kind, event.state.rule) {
                (EventKind::Opened, _) => "Открыт сигнал",
                (EventKind::Recovered, Rule::PublicPageLoss) => {
                    "В текущей проверяемой выборке нет подтверждённых потерь"
                }
                (EventKind::Recovered, _) => "Подтверждено восстановление",
            };
            write!(
                body,
                r#"<article data-testid="signal-event" data-kind="{}"><h3>{}: {}</h3><p>{}</p>"#,
                event.kind.code(),
                label,
                title(event.state.rule),
                reason(event.state.result.reason)
            )
            .expect("String");
            if event.kind == EventKind::Recovered && event.state.rule == Rule::PublicPageLoss {
                body.push_str("<p>Восстановление ранее проверенных URL не утверждается.</p>");
            }
            evidence(&mut body, &event.state);
            body.push_str("</article>");
        }
        if let Some(next) = report.next {
            write!(body, r#"<p><a data-testid="signal-next" href="/admin/audience/signals?days={days}&amp;before={next}">Предыдущие записи</a></p>"#).expect("String");
        }
    }
    write!(body, r#"<p><a href="/admin/audience/signals?days={days}">К последним проверкам</a></p></section>"#).expect("String");
    body.push_str(r#"<section class="book-section-card"><h2>Правила проверки</h2>
<p>Для HTML-запросов, чтения и карточек сравниваются соседние равные интервалы полностью закрытых часов UTC. Для карточек дополнительно проходит 15 минут на доставку наблюдений. Google и Яндекс сохраняют собственные даты отчётности; они не смешиваются с часами посещений.</p>
<p>Сигнал падения требует изменения не менее 30% и достаточного предыдущего значения: 100 для HTML-запросов, кликов поиска и показов карточек; 50 для подтверждённого чтения. Число завершений означает первое подтверждённое завершение главы на пару «аккаунт — книга», а не прочтение всей книги.</p>
<p>Малое число показов определяется только при не менее 20 подтверждённых появлений карточек в документе, покрытии наблюдателем не ниже 80% и менее 5 показов. Неизвестные книги и жанры не превращаются в нулевые значения.</p>
<p>Подтверждение сборщика относится только к зарегистрированным исполнителям. Оно не доказывает охват всей установки: неизвестные процессы и незавершённые интервалы не считаются измеренными.</p>
<p>Периоды проверяются по очереди примерно раз в две минуты. Результат старше пяти минут считается неизвестным. Повторные сообщения об одном эпизоде ограничены интервалом 24 часа; действующее состояние остаётся видимым. Пропуск данных, выключение источника и смена его настроек не закрывают прежний эпизод как восстановленный.</p>
<p>Журнал хранит до 180 дней изменений; незакрытые состояния сохраняются. Эти сигналы доступны здесь и не отправляются во внешние каналы, не меняют рекомендации и не управляют рекламой.</p></section></div></section>"#);
    layout(
        "Сигналы аудитории",
        "Закрытый журнал качества данных и изменений.",
        &body,
    )
}
fn number(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "неизвестно".into())
}
fn evidence(body: &mut String, state: &SignalState) {
    let e = state.evidence;
    write!(
        body,
        "<p>Проверено: {}. Период: {}.</p>",
        crate::flow_runtime::utc(state.checked_at_ms / 1000),
        escape_html(&state.window.label)
    )
    .expect("String");
    if !e.available {
        body.push_str("<p>Данные источника не получены; нули не подставлены.</p>");
        return;
    }
    let units = match state.rule {
        Rule::GoogleQuality | Rule::GoogleClicks | Rule::YandexQuality | Rule::YandexClicks => {
            "дней"
        }
        Rule::PublicPageLoss => "адресов текущей выборки",
        _ => "закрытых часов",
    };
    write!(body, "<dl><dt>Текущее значение</dt><dd>{}</dd><dt>Предыдущее значение</dt><dd>{}</dd><dt>Подтверждено {units}</dt><dd>{} / {}; ранее {} / {}</dd></dl>",
        number(e.current), number(e.previous), e.current_units, e.expected_units, e.previous_units, e.expected_units).expect("String");
    if e.known_discards > 0 {
        write!(
            body,
            "<p>Часов с зарегистрированной потерей: {}.</p>",
            e.known_discards
        )
        .expect("String");
    }
    if e.source_failed {
        body.push_str("<p>Есть зарегистрированная ошибка источника.</p>");
    }
    if matches!(
        state.rule,
        Rule::ExposureQuality
            | Rule::ExposureChange
            | Rule::UnderexposedBooks
            | Rule::UnderexposedGenres
    ) {
        let ack = |value: Option<i32>| {
            value
                .map(|value| format!("{}.{:01}%", value / 10, value % 10))
                .unwrap_or_else(|| "неизвестно".into())
        };
        write!(body, "<p>Подтверждённые появления в документе: {} из {}; ранее {} из {}. Покрытие наблюдателем: {}; ранее {}.</p>",
            e.current_sample, e.current_opportunities, e.previous_sample, e.previous_opportunities, ack(e.current_ack_per_mille), ack(e.previous_ack_per_mille)).expect("String");
    }
    if e.unknown_entities > 0 {
        write!(
            body,
            "<p>Объектов с неизвестным результатом: {}.</p>",
            e.unknown_entities
        )
        .expect("String");
    }
    if e.limited_units > 0 || e.omitted_rows > 0 {
        write!(
            body,
            "<p>Дней с ограниченным списком страниц: {}; пропущенных строк: {}.</p>",
            e.limited_units, e.omitted_rows
        )
        .expect("String");
    }
}
fn title(rule: Rule) -> &'static str {
    match rule {
        Rule::HtmlQuality => "Полнота HTML-измерения",
        Rule::HtmlChange => "Изменение HTML-запросов",
        Rule::ReadingStarts => "Подтверждённые начала глав",
        Rule::ReadingCompletions => "Первое завершение главы в книге",
        Rule::GoogleQuality => "Качество данных Google",
        Rule::GoogleClicks => "Изменение кликов Google",
        Rule::YandexQuality => "Качество данных Яндекса",
        Rule::YandexClicks => "Изменение кликов Яндекса",
        Rule::ExposureQuality => "Полнота наблюдения карточек",
        Rule::ExposureChange => "Изменение наблюдаемых показов",
        Rule::UnderexposedBooks => "Книги с малым числом показов",
        Rule::UnderexposedGenres => "Жанры с малым числом показов",
        Rule::PublicPageLoss => "Недоступные публичные страницы",
    }
}
fn status_label(status: Status) -> &'static str {
    match status {
        Status::Active => "Требует проверки",
        Status::Unknown => "Неизвестно",
        Status::Healthy => "Условие сигнала не обнаружено",
        Status::Disabled => "Источник выключен",
    }
}
fn reason(reason: Reason) -> &'static str {
    match reason {
        Reason::WithinThreshold => "Проверенное значение не пересекло порог сигнала.",
        Reason::RecordedDrop => {
            "Зафиксировано падение относительно предыдущего сопоставимого периода."
        }
        Reason::RecordedGrowth => {
            "Зафиксирован рост относительно предыдущего сопоставимого периода."
        }
        Reason::MissingMeasurement => "Подтверждённого измерения недостаточно.",
        Reason::PartialCoverage => "Подтверждена только часть периода.",
        Reason::SourceFailure => "Источник зарегистрировал ошибку.",
        Reason::DiscardedObservations => "Зарегистрированы потери наблюдений.",
        Reason::LimitedPages => "Список страниц ограничен или содержит пропущенные строки.",
        Reason::SmallBaseline => "Выборка слишком мала для достоверного сравнения.",
        Reason::SourceUnavailable => "Источник или свежий результат недоступен.",
        Reason::SourceDisabled => "Сбор или импорт этого источника выключен.",
        Reason::LowExposure => "Есть объекты с подтверждённо малым числом показов.",
        Reason::ObserverCoverage => "Доля подтверждённых наблюдений ниже порога.",
        Reason::PublicPageLoss => {
            "Ранее доступный адрес текущей выборки вернул другой HTTP-статус."
        }
    }
}
fn source_link(rule: Rule, days: i32) -> String {
    match rule {
        Rule::HtmlQuality | Rule::HtmlChange => format!("/admin/audience?days={days}"),
        Rule::ReadingStarts | Rule::ReadingCompletions => {
            format!("/admin/audience/reading?days={days}")
        }
        Rule::GoogleQuality | Rule::GoogleClicks => {
            format!("/admin/audience/seo?days={days}&amp;provider=google")
        }
        Rule::YandexQuality | Rule::YandexClicks => {
            format!("/admin/audience/seo?days={days}&amp;provider=yandex")
        }
        Rule::PublicPageLoss => "/admin/audience/pages".into(),
        _ => format!("/admin/audience/exposure?days={days}&amp;window=closed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_recommendations::services::flow_signals::{
        Episode, Evaluation, Evidence, SignalWindow,
    };
    #[test]
    fn missing_and_stale_do_not_claim_recovery_and_persisted_labels_are_escaped() {
        let missing = page(7, None).0;
        assert_eq!(missing.matches(r#"data-testid="signal-state""#).count(), 13);
        assert!(!missing.contains(r#"data-state="healthy""#));
        let state = SignalState {
            rule: Rule::HtmlQuality,
            days: 7,
            scope: "a".repeat(64),
            window: SignalWindow {
                until_epoch: Some(3600),
                label: "<script>bad</script>".into(),
            },
            result: Evaluation::new(Status::Healthy, Reason::WithinThreshold),
            evidence: Evidence::missing(true),
            episode: Episode {
                reason: Some(Reason::DiscardedObservations),
                ..Episode::default()
            },
            checked_at_ms: 1,
            last_epoch: 1,
        };
        let report = SignalReport {
            checked_at_ms: 600_001,
            days: 7,
            states: vec![state],
            events: vec![],
            next: Some(9),
        };
        let html = page(7, Some(&report)).0;
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
        assert!(!html.contains("<script>bad"));
        assert!(html.contains("signal-open-episode"));
        assert!(html.contains("days=7&amp;before=9"));
        assert!(!html.contains(r#"data-state="healthy""#));
    }
}
