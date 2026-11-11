//! Aggregate cohorts with explicit unknown and parallel outcomes.
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::repositories::flow_capture::FlowCaptureHealth;
use tor_recommendations::services::flow_funnel::{
    FlowFunnelCounts, FlowFunnelPeriod, FlowFunnelReport,
};

pub(in crate::auth::audience_http) fn page(
    days: i32,
    report: Option<&FlowFunnelReport>,
    capture: FlowCaptureHealth,
) -> Html<String> {
    let mut body = format!(
        "<section class=\"section\"><div class=\"container author-width\">\
         <p class=\"eyebrow\">Администратору сайта</p><h1>Путь от визита к чтению</h1>\
         <p><a href=\"/admin/audience?days={days}\">Аудитория</a> · \
         <a href=\"/admin/audience/visits?days={days}\">Визиты и входы</a> · \
         <a href=\"/admin/audience/books?days={days}\">Книги</a></p>\
         <p>Как прежние наблюдения посещений книг были связаны с подтверждённым чтением, \
         сохранениями, подписками и возвратами. Только зарегистрированные аккаунты \
         с отдельным согласием на измерение визитов.</p>\
         <form class=\"author-form\" method=\"get\" action=\"/admin/audience/funnel\">\
         <label for=\"flow-funnel-days\">Длительность периода</label>\
         <select id=\"flow-funnel-days\" name=\"days\">"
    );
    for (value, label) in [
        (1, "1 день"),
        (7, "7 дней"),
        (30, "30 дней"),
        (90, "90 дней"),
    ] {
        let selected = if days == value { " selected" } else { "" };
        write!(body, "<option value=\"{value}\"{selected}>{label}</option>").expect("String");
    }
    body.push_str(
        "</select><p><button class=\"button\" type=\"submit\">Показать</button></p></form>",
    );
    super::retirement::notice(
        &mut body,
        "связь посещения с чтением, сохранением, подпиской и возвращением",
    );
    body.push_str(
        "<section class=\"book-section-card\"><h2>Что считается одним посещением книги</h2>\
         <p>Один аккаунт, одна книга, один наблюдаемый визит. Повторные открытия \
         карточки и нескольких глав внутри визита не увеличивают основание. \
         Визит заканчивается после 30 минут без наблюдаемой активности.</p>\
         <p>Открытие читалки не означает чтение. Система подтверждает первое начало \
         и первое завершение каждой главы по действиям в читалке. Повторное чтение \
         уже учтённой главы не создаёт таких событий снова. Завершение относится \
         к тому же посещению книги, к которому привязано начало этой главы, даже если оно произошло позже. \
         Здесь считается наличие завершённой главы в книге, а не завершение всей книги.</p>\
         <p>Сохранение в библиотеку и подписка — самостоятельные результаты. \
         Читатель может сделать их до начала чтения. Эти числа нельзя складывать \
         или трактовать как обязательные последовательные шаги.</p></section>",
    );
    match report {
        Some(report) => {
            write!(body, "<p>Расчёт: {} UTC. Все границы периода указаны в UTC; текущий день ещё не завершён.</p>",
                escape_html(&report.measured_at_utc)).expect("String");
            period(
                &mut body,
                "current",
                super::retirement::CURRENT,
                &report.current,
            );
            period(
                &mut body,
                "previous",
                super::retirement::PREVIOUS,
                &report.previous,
            );
            worker(&mut body, report);
        }
        None => body.push_str(
            "<p role=\"alert\" data-testid=\"flow-funnel-unavailable\">\
             Сводка пути к чтению временно недоступна. Нулевые значения не подставляются.</p>",
        ),
    }
    limitations(&mut body);
    write!(body,
        "<section class=\"book-section-card\"><h2>Ошибки записи действий в этом процессе</h2>\
         <p>Счётчики с запуска процесса: связь начала и завершения чтения — {}; сохранения и подписки — {}; \
         активность чтения — {}; очистка по сроку хранения — {}. \
         Они не описывают другие процессы и не доказывают полноту выбранного периода.</p>\
         <p><a href=\"/admin/audience/visits?days={days}\">Состояние очереди визитов</a></p></section>",
        capture.read_failures, capture.action_failures, capture.activity_failures, capture.maintenance_failures
    ).expect("String");
    if capture.read_failures > 0
        || capture.action_failures > 0
        || capture.activity_failures > 0
        || capture.maintenance_failures > 0
    {
        body.push_str("<p role=\"alert\">Есть ошибки необязательного измерения. Отсутствие результата может означать пропуск данных.</p>");
    }
    body.push_str("<p><a href=\"/me\">В личный кабинет</a></p></div></section>");
    layout(
        "Путь от визита к чтению",
        "Закрытая сводка наблюдаемых посещений книг.",
        &body,
    )
}

fn number(value: Option<i64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "Нет измерения".to_owned())
}

fn period(body: &mut String, key: &str, title: &str, data: &FlowFunnelPeriod) {
    write!(
        body,
        "<section class=\"book-section-card\" data-testid=\"flow-funnel-{key}\" data-measurement=\"legacy-consented\"><h2>{title}</h2>\
         <p>С {} до {} UTC, конец не включён. Все результаты отнесены к периоду первого \
         открытия книги в соответствующем визите.</p>",
        escape_html(&data.from_utc),
        escape_html(&data.until_utc)
    )
    .expect("String");
    if let Some(counts) = &data.counts {
        counts_list(body, counts);
    } else {
        body.push_str("<p>Нет сохранённых посещений книг за этот период. Это не доказывает отсутствие посетителей или чтения.</p>");
    }
    write!(body,
        "<dl><dt>Необработанные наблюдения прежней модели</dt><dd data-metric=\"pending-observations\">{}</dd>\
         <dt>Открытия книги без сохранённой связи с визитом</dt><dd data-metric=\"unmatched-observations\">{}</dd></dl>",
        number(data.pending_book_observations), number(data.unmatched_book_observations)
    ).expect("String");
    body.push_str("<p>Эти два показателя относятся к отдельным наблюдениям страниц, а не к посещениям книг. Они доступны только в пределах полного сохраняемого окна подробностей.</p>");
    if !data.rows.is_empty() {
        body.push_str("<h3>Источники наблюдаемых посещений книг</h3>");
    }
    for row in &data.rows {
        write!(
            body,
            "<article class=\"book-section-card\" data-source=\"{}\" data-discovery=\"{}\">\
             <h4>{}</h4><p>Переход к книге: {}.</p>",
            row.acquisition_hint,
            row.discovery_kind,
            source(row.acquisition_hint),
            discovery(row.discovery_kind)
        )
        .expect("String");
        counts_list(body, &row.counts);
        body.push_str("</article>");
    }
    body.push_str("</section>");
}

fn counts_list(body: &mut String, c: &FlowFunnelCounts) {
    body.push_str("<dl>");
    for (key, label, count) in [
        ("cohorts", "Наблюдаемые посещения книг", c.cohorts),
        ("detail-opens", "С открытием карточки книги", c.detail_opens),
        (
            "reader-opens",
            "С разрешённым открытием читалки",
            c.reader_opens,
        ),
        ("starts", "С подтверждённым началом главы", c.starts),
        (
            "completions",
            "С подтверждённым завершением начатой главы",
            c.completions,
        ),
        ("saves", "С новым сохранением книги в библиотеку", c.saves),
        ("follows", "С новой подпиской на книгу", c.follows),
        (
            "new-readers",
            "С первым известным подтверждённым началом этой книги",
            c.new_readers,
        ),
        (
            "d7-anchors",
            "Первое начало после согласия, связанное с визитом",
            c.d7_anchors,
        ),
        (
            "d7-pending",
            "Из них незавершённые исторические наблюдения",
            c.d7_pending,
        ),
        (
            "d7-returned",
            "Из них подтверждено чтение книги на 7-й день",
            c.d7_returned,
        ),
        (
            "d7-unknown",
            "Из них результат 7-го дня неизвестен",
            c.d7_unknown,
        ),
    ] {
        write!(
            body,
            "<dt>{label}</dt><dd data-metric=\"{key}\">{count}</dd>"
        )
        .expect("String");
    }
    body.push_str("</dl>");
}

fn source(code: i16) -> &'static str {
    match code {
        0 => "Первый сигнал визита: внешний поиск",
        1 => "Первый сигнал визита: внешний сайт",
        _ => "Первый источник визита неизвестен или прямой вход",
    }
}

fn discovery(code: i16) -> &'static str {
    match code {
        0 => "подтверждённая ссылка рекомендаций",
        1 => "подтверждённая ссылка каталога",
        2 => "подтверждённая ссылка поиска или фильтров каталога",
        _ => "нет подтверждённой внутренней ссылки",
    }
}

fn worker(body: &mut String, report: &FlowFunnelReport) {
    body.push_str("<section class=\"book-section-card\"><h2>Обработка наблюдений</h2><p>Состояние последнего записавшего отчёт процесса. Оно не подтверждает полноту сбора на всём сайте.</p>");
    match &report.worker_last_success_utc {
        Some(at) => {
            write!(
                body,
                "<p>Последняя успешная обработка: {} UTC.</p>",
                escape_html(at)
            )
            .expect("String");
        }
        None => body.push_str("<p role=\"alert\">Успешная обработка пока не подтверждена.</p>"),
    }
    if !report.worker_collecting {
        body.push_str("<p data-testid=\"flow-funnel-collection-off\">Последний сохранённый режим: сбор выключен. Старые наблюдения могут оставаться в отчёте.</p>");
    }
    if report.worker_backlog {
        body.push_str("<p role=\"alert\" data-testid=\"flow-funnel-backlog\">Есть необработанные данные. Итоги ещё могут измениться.</p>");
    }
    body.push_str(
        "<p>В прежней модели задержка обработки открытия книги составляла не менее 5 минут и 5 секунд. \
        Она нужна для запоздавших наблюдений. Ошибки, выключенный сбор и утраченные наблюдения \
        не восстанавливаются из истории посещений.</p></section>",
    );
}

fn limitations(body: &mut String) {
    body.push_str(
        "<section class=\"book-section-card\"><h2>Как читать результаты</h2>\
         <p>Внешний источник — подсказка браузера на первом наблюдаемом запросе визита. \
         Она может отсутствовать или быть подменена. Внутренняя ссылка подтверждается \
         сервером для текущего аккаунта и согласия; адрес страницы сам по себе её не доказывает.</p>\
         <p>Начало главы, сохранение и подписка связываются с последним подходящим \
         посещением книги за предшествующие 7 суток. Завершение связывается с началом \
         той же главы в пределах 14 суток после него. Более поздние результаты \
         не приписываются этому посещению.</p>\
         <p>Возврат на 7-й день — положительное продвижение по этой книге в точную \
         календарную дату UTC через семь дней после первого подтверждённого начала \
         после текущего согласия. Если первое начало не связано с визитом, позднее \
         чтение не становится новым основанием. Отсутствие сохранённого продвижения \
         означает неизвестный результат; оно не доказывает, что читатель не вернулся. \
         Возврат на 30-й день по источнику здесь не рассчитывается.</p>\
         <p>Подробности хранятся не дольше 30 суток от начала визита; обезличенные итоги — \
         до 180 дней. Поэтому поздний результат может остаться неизвестным. В прежней модели при отзыве \
         согласия ещё сохраняемые персональные наблюдения и их вклад удалялись. \
         После удаления подробных связей поздно поступившие факты чтения уже не \
         пересчитывают архивные итоги по источникам. \
         Сравнение периодов описывает сохранённые наблюдения, а не изменение всей аудитории. \
         На небольшом числе посещений устойчивый вывод делать рано.</p></section>"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_evidence_is_not_reported_as_zero_or_a_negative_return() {
        let empty = FlowFunnelPeriod {
            from_utc: "2026-11-10T00:00:00Z".into(),
            until_utc: "\"><script>bad()</script>".into(),
            counts: None,
            pending_book_observations: None,
            unmatched_book_observations: None,
            rows: Vec::new(),
        };
        let report = FlowFunnelReport {
            days: 7,
            measured_at_utc: "2026-11-10T12:00:00Z".into(),
            current: empty.clone(),
            previous: empty,
            worker_last_success_utc: None,
            worker_collecting: false,
            worker_backlog: true,
        };
        let html = page(7, Some(&report), FlowCaptureHealth::default()).0;
        assert!(html.contains("Нет измерения"));
        assert!(!html.contains("data-metric=\"starts\">0"));
        assert!(!html.contains("<script>bad()"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("flow-funnel-backlog"));
        assert!(html.contains("не доказывает, что читатель не вернулся"));
        assert!(page(7, None, FlowCaptureHealth::default())
            .0
            .contains("flow-funnel-unavailable"));
    }
}
