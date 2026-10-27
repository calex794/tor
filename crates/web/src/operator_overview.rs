//! Read-only system overview with the same independent operator password as traces.
//! No business records or private URLs; unavailable observations stay unknown.
use crate::{cover_worker, observability, operator_traces, ril, worker_health, AppState};
use axum::{
    extract::State,
    http::HeaderMap,
    response::{Html, Response},
};
use std::{fmt::Write, time::Duration};
use tor_ingest::repositories::cover_jobs::{CoverQueueCounts, PgCoverJobs};
use crate::cover_worker::CoverRunnerSnapshot;
use tor_observability::services::{
    process::{self, ProcessSnapshot},
    publisher::PublisherCounts,
    HttpSummary,
};
use tor_traffic_behavior::{
    repositories::{PgRilRollups, RilQueueCounts},
    services::PgPumpSnapshot,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PgHealth {
    NotConfigured,
    Ready,
    Unavailable,
}

struct Overview {
    pg: PgHealth,
    http: HttpSummary,
    process: ProcessSnapshot,
    ril_dropped: Option<(u64, u64)>,
    trace_store_configured: bool,
    trace_delivery: Option<PublisherCounts>,
    ril_queue: Option<RilQueueCounts>,
    cover_queue: Option<CoverQueueCounts>,
    ril_pump: Option<PgPumpSnapshot>,
    cover_runner: Option<CoverRunnerSnapshot>,
}

async fn inspect(postgres: Option<tor_books::repositories::PgCatalog>) -> Overview {
    let mut state = Overview {
        pg: PgHealth::NotConfigured,
        http: observability::http_summary(),
        process: process::capture(),
        ril_dropped: ril::dropped_telemetry_counters(),
        trace_store_configured: std::env::var_os("TOR_TRACE_STORE_SOCKET")
            .is_some_and(|value| std::path::Path::new(&value).is_absolute()),
        trace_delivery: observability::trace_delivery_counts(),
        ril_queue: None,
        cover_queue: None,
        ril_pump: ril::pump_health_snapshot(),
        cover_runner: cover_worker::cover_runner_snapshot(),
    };
    let Some(catalog) = postgres else {
        return state;
    };
    let timeout = Duration::from_millis(950);
    if !matches!(
        tokio::time::timeout(timeout, catalog.probe()).await,
        Ok(Ok(()))
    ) {
        state.pg = PgHealth::Unavailable;
        return state;
    }
    state.pg = PgHealth::Ready;
    let pool = catalog.pool();
    let ril_repo = PgRilRollups::new(pool.clone());
    let cover_repo = PgCoverJobs::new(pool);
    let ril_future = tokio::time::timeout(timeout, ril_repo.queue_counts());
    let cover_future = tokio::time::timeout(timeout, cover_repo.queue_counts());
    let (ril, covers) = tokio::join!(ril_future, cover_future);
    state.ril_queue = ril.ok().and_then(Result::ok);
    state.cover_queue = covers.ok().and_then(Result::ok);
    state
}

fn unmeasured() -> &'static str {
    "Нет измерения"
}

fn render(snapshot: &Overview) -> Html<String> {
    let pg_text = match snapshot.pg {
        PgHealth::NotConfigured => "Не подключена (демонстрационный режим)",
        PgHealth::Ready => "Проверка готовности прошла",
        PgHealth::Unavailable => "Недоступна или проверка превысила лимит времени",
    };
    let resident = snapshot
        .process
        .resident_kib
        .map(|kb| format!("{:.1} МиБ", kb as f64 / 1024.0))
        .unwrap_or_else(|| unmeasured().to_owned());
    let cpu = snapshot
        .process
        .cpu_seconds
        .map(|value| format!("{value:.2} с"))
        .unwrap_or_else(|| unmeasured().to_owned());
    let cpu_share = snapshot
        .process
        .cpu_core_percent
        .map(|value| format!("{value:.1}% от одного ядра"))
        .unwrap_or_else(|| "Нет измерения (нужен повторный просмотр)".to_owned());

    let ril_queue = match snapshot.ril_queue {
        None => format!("<p>{}</p>", unmeasured()),
        Some(rows) => format!(
            "<dl><dt>Ожидают</dt><dd>{}</dd><dt>В работе</dt><dd>{}</dd><dt>Неисправимые</dt><dd>{}</dd><dt>Ожидают более 5 минут</dt><dd>{}</dd><dt>Истекла аренда</dt><dd>{}</dd><dt>Три и более попытки</dt><dd>{}</dd></dl>",
            rows.pending, rows.running, rows.dead,
            rows.overdue_pending, rows.expired_lease, rows.repeated_attempts
        ),
    };
    let covers = match snapshot.cover_queue {
        None => format!("<p>{}</p>", unmeasured()),
        Some(rows) => format!(
            "<dl><dt>Ожидают</dt><dd>{}</dd><dt>В работе</dt><dd>{}</dd><dt>Готовы к закреплению</dt><dd>{}</dd><dt>Неисправимые</dt><dd>{}</dd><dt>Ожидают более 5 минут</dt><dd>{}</dd><dt>Истекла аренда</dt><dd>{}</dd><dt>Три и более попытки</dt><dd>{}</dd></dl>",
            rows.pending, rows.running, rows.ready, rows.dead,
            rows.overdue_pending, rows.expired_lease, rows.repeated_attempts
        ),
    };
    let pump_status = worker_health::ril_pump(snapshot.ril_pump, snapshot.ril_queue);
    let cover_status = worker_health::cover_runner(snapshot.cover_runner, snapshot.cover_queue);
    let pump_text = pump_status.text;
    let cover_text = cover_status.text;
    let pump_metrics = match snapshot.ril_pump {
        None => format!("<p>{}</p>", unmeasured()),
        Some(pump) => {
            let last_cycle = pump
                .age_since_last_completion_s
                .map(|age| format!("{age} с назад"))
                .unwrap_or_else(|| "Не было завершённых циклов".to_owned());
            format!(
                "<dl><dt>Завершено циклов</dt><dd>{}</dd><dt>Циклы с ошибками</dt><dd>{}</dd><dt>Ошибок подряд</dt><dd>{}</dd><dt>Последний цикл</dt><dd>{}</dd></dl>",
                pump.cycles, pump.failed_cycles, pump.consecutive_failures, last_cycle,
            )
        }
    };
    let pump_role = if pump_status.warning {
        "alert"
    } else {
        "status"
    };
    let cover_metrics = match snapshot.cover_runner {
        None => "<p>Локальный исполнитель не включён или не запускался.</p>".to_owned(),
        Some(runner) => {
            let last = runner.since_completion_s
                .map(|seconds| format!("{seconds} с назад"))
                .unwrap_or_else(|| "Ещё не завершал цикл".to_owned());
            format!(
                "<dl><dt>Завершено циклов</dt><dd>{}</dd><dt>Прикреплено обложек</dt><dd>{}</dd><dt>Устаревшие задания</dt><dd>{}</dd><dt>Повторы/карантин</dt><dd>{}</dd><dt>Технические ошибки</dt><dd>{}</dd><dt>Превышения времени</dt><dd>{}</dd><dt>Ошибки подряд</dt><dd>{}</dd><dt>Последний цикл</dt><dd>{}</dd></dl>",
                runner.cycles, runner.attached, runner.superseded,
                runner.retried_or_quarantined, runner.errors, runner.timeouts,
                runner.consecutive_errors, last
            )
        }
    };
    let cover_role = if cover_status.warning {
        "alert"
    } else {
        "status"
    };
    let ril_telemetry = snapshot.ril_dropped
        .map(|(ordinary, path)| format!(
            "<dl><dt>Потеряно технических событий</dt><dd>{ordinary}</dd><dt>Потеряно событий путей</dt><dd>{path}</dd></dl>"
        ))
        .unwrap_or_else(|| format!("<p>{}</p>", unmeasured()));
    let mut exporters = format!(
        "<p>Архив SQLite: {}</p>",
        if snapshot.trace_store_configured {
            "путь настроен (не подтверждает доступность службы)"
        } else {
            "выключен"
        },
    );
    if let Some(counters) = snapshot.trace_delivery {
        write!(
            exporters,
            "<dl><dt>Подтверждено записей</dt><dd>{}</dd><dt>Ошибки отправки</dt><dd>{}</dd><dt>Сброшено при переполнении</dt><dd>{}</dd></dl>",
            counters.sent, counters.failed, counters.dropped
        )
        .expect("write String");
    } else {
        write!(exporters, "<p>{}</p>", unmeasured()).expect("write String");
    }

    let html = format!(
        "<div class=\"grid\">
        <section class=\"card\" aria-labelledby=\"pg\"><h2 id=\"pg\">PostgreSQL</h2><p>{pg_text}</p><p class=\"muted\">Проверяется фактическая схема и доступность данных сайта. Это не измерение задержек всех запросов.</p></section>
        <section class=\"card\" aria-labelledby=\"system\"><h2 id=\"system\">Ресурсы процесса TOR</h2><dl><dt>Оперативная память (RSS)</dt><dd>{resident}</dd><dt>Накопленное время CPU</dt><dd>{cpu}</dd><dt>Загрузка CPU между просмотрами</dt><dd>{cpu_share}</dd></dl><p class=\"muted\">Только процесс сайта, не весь сервер. На системах без /proc данные недоступны; 100% означает одно полностью занятое ядро.</p></section>
        <section class=\"card\" aria-labelledby=\"http\"><h2 id=\"http\">HTTP с запуска процесса</h2><dl><dt>Завершено запросов</dt><dd>{}</dd><dt>Ответы 4xx</dt><dd>{}</dd><dt>Ответы 5xx</dt><dd>{}</dd><dt>Медленнее одной секунды</dt><dd>{}</dd></dl><p class=\"muted\">Коды 4xx могут быть обычными отказами доступа; это не обязательно ошибка сервера.</p></section>
        <section class=\"card\" aria-labelledby=\"ril\"><h2 id=\"ril\">RIL: маршруты и потери</h2><dl><dt>Известных методов маршрутов</dt><dd>{}</dd><dt>Версия классификации</dt><dd>{}</dd></dl>{ril_telemetry}<p class=\"muted\">RIL классифицирует маршруты, но не выдаёт права доступа. Потери событий не означают потерю HTTP-запросов.</p></section>
        <section class=\"card\" aria-labelledby=\"jobs\"><h2 id=\"jobs\">Очередь аналитики RIL</h2><p role=\"{pump_role}\">{pump_text}</p>{pump_metrics}{ril_queue}<p class=\"muted\">Нет завершённого цикла 90 секунд — возможная остановка. Просрочка очереди: больше 5 минут после поступления и наступления срока запуска. Активность сборщика относится только к этому веб-процессу.</p></section>
        <section class=\"card\" aria-labelledby=\"images\"><h2 id=\"images\">Очередь обработки обложек</h2><p role=\"{cover_role}\">{cover_text}</p>{cover_metrics}{covers}<p class=\"muted\">Показатели исполнителя относятся только к процессу TOR. Просроченная аренда не доказывает остановку другого процесса.</p></section>
        <section class=\"card\" aria-labelledby=\"trace\"><h2 id=\"trace\">Доставка трасс</h2>{exporters}<p class=\"muted\">Счётчики накоплены с запуска веб-процесса. Отсутствующие измерения не заменяются нулём.</p></section>
        </div>",
        snapshot.http.requests,
        snapshot.http.client_errors,
        snapshot.http.server_errors,
        snapshot.http.slower_than_one_second,
        tor_route_intelligence::registry().len(),
        tor_route_intelligence::RIL_CLASSIFIER_VERSION,
    );
    Html(include_str!("../templates/operator_overview.html").replace("<!--SUMMARY-->", &html))
}

async fn respond(
    headers: HeaderMap,
    secret: Option<&str>,
    postgres: Option<tor_books::repositories::PgCatalog>,
) -> Response {
    // Authorize before reading process metrics or probing PostgreSQL.
    if !operator_traces::authorized(&headers, secret) {
        return operator_traces::denied();
    }
    let summary = inspect(postgres).await;
    operator_traces::secure(render(&summary))
}

pub(crate) async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let secret = std::env::var("TOR_OBS_OPERATOR_PASSWORD").ok();
    respond(headers, secret.as_deref(), state.postgres).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{header, HeaderValue},
    };
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    fn credentials(password: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!(
                "Basic {}",
                STANDARD.encode(format!("operator:{password}"))
            ))
            .unwrap(),
        );
        headers
    }

    #[tokio::test]
    async fn no_operator_password_or_wrong_user_is_denied_before_probe() {
        let secret = "a".repeat(48);
        for (headers, config) in [
            (HeaderMap::new(), Some(secret.as_str())),
            (credentials("wrong"), Some(secret.as_str())),
            (credentials(&secret), None),
        ] {
            let response = respond(headers, config, None).await;
            assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }

    #[tokio::test]
    async fn authorized_demo_does_not_invent_postgres_or_queue_status() {
        let secret = "a".repeat(48);
        let response = respond(credentials(&secret), Some(&secret), None).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let html = to_bytes(response.into_body(), 60_000).await.unwrap();
        let body = String::from_utf8(html.to_vec()).unwrap();
        assert!(body.contains("Не подключена (демонстрационный режим)"));
        assert!(body.contains("Нет измерения"));
        assert!(body.contains("Общая сводка"));
        assert!(!body.contains("user_id"));
        assert!(!body.contains("SQL SELECT"));
    }

    #[test]
    fn combined_view_distinguishes_known_zero_and_unknown() {
        let data = Overview {
            pg: PgHealth::Unavailable,
            http: HttpSummary {
                requests: 13,
                server_errors: 2,
                client_errors: 1,
                slower_than_one_second: 3,
            },
            process: ProcessSnapshot::default(),
            ril_dropped: Some((0, 0)),
            trace_store_configured: false,
            trace_delivery: None,
            ril_queue: None,
            cover_queue: Some(CoverQueueCounts {
                pending: 1,
                running: 2,
                ready: 0,
                dead: 1,
                overdue_pending: 1,
                expired_lease: 1,
                repeated_attempts: 3,
            }),
            ril_pump: None,
            cover_runner: None,
        };
        let text = render(&data).0;
        assert!(text.contains("Недоступна или проверка"));
        assert!(text.contains("Очередь обработки обложек"));
        assert!(text.contains("В работе</dt><dd>2"));
        assert!(text.contains("HTTP с запуска процесса"));
        assert!(text.contains("Нет измерения"));
        assert!(!text.contains("secret"));
    }
}
