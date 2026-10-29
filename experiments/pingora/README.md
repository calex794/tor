# PINGORA-TOR-01 — изолированный опыт

Это самостоятельный Cargo-проект вне рабочего TOR. Рабочий Nginx, RIL, база и VPS не изменяются.

Nginx уже принимает HTTPS и фильтрует запросы. RIL классифицирует маршруты и измеряет стоимость. Pingora здесь не заменяет права доступа, RIL, кеш или ограничения запросов.

## Исправление аварийного запуска

У Pingora 0.9.0 обнаружена несовместимость с Tokio 1.54: при первом подключении создаётся нулевой буфер, вызывающий панику. См. https://github.com/cloudflare/pingora/issues/1059. В этом изолированном Cargo-проекте Tokio временно закреплён на 1.53.2. Это не исправляет Pingora и не позволяет переносить код в рабочую сборку. После исправления Pingora закрепление следует убрать.

## Проверка сборки и HTTP

Нужны Rust stable, pkg-config, OpenSSL headers и локальный HTTP-источник на 127.0.0.1:6194.

~~~sh
cd experiments/pingora
cargo generate-lockfile
cargo test --locked --all-targets
PINGORA_EXPERIMENT=1 PINGORA_EXPERIMENT_BIND=127.0.0.1:6193 PINGORA_EXPERIMENT_UPSTREAM=127.0.0.1:6194 cargo run --locked
~~~

Опыт не запускается без PINGORA_EXPERIMENT=1 и отказывается принимать публичные адреса или внешние источники.

## Три схемы измерения в CI

В GitHub Actions сначала выполняется `cargo build --release --locked`, чтобы Pingora и Axum не сравнивались с оптимизированным системным Nginx в отладочном режиме. Предыдущий запуск №40773654332 использовал debug-сборку: его цифры нельзя принимать за итог. Затем создаётся временный Docker-контейнер с жёстким ограничением 1 vCPU / 1 GiB RAM. Внутри контейнера один и тот же искусственный Axum-сервер, Pingora 0.9.0 и Nginx. Генератор нагрузки wrk запускается вне контейнера, чтобы не отнимать квоту процессора у серверов.

| Схема | Адрес на тестовой машине |
| --- | --- |
| Nginx → Axum | 127.0.0.1:6195 |
| Pingora → Axum | 127.0.0.1:6193 |
| Nginx → Pingora → Axum | 127.0.0.1:6196 |

Контейнер и все HTTP-серверы доступны только через 127.0.0.1. Перед нагрузкой проверяются HTTP 200, 404, 429, 503. Затем измеряются три публичных маршрута — каталог, чтение и API. Для каждого сочетания проводится прогрев и пять проходов по десять секунд; порядок схем меняется между проходами. Соединения HTTP/1.1 с повторным использованием, 16 параллельных соединений, один поток wrk.

Измерения: запросы в секунду, p50/p95/p99, ошибки, примеры расхода памяти и процессора контейнера. Данные в артефакте tor-pingora-1cpu-1gib-comparison: measurements.txt, summary.csv (медианы пяти измерений), resource-samples.txt, полные журналы wrk и server.log. Значения docker stats — отдельные мгновенные снимки контейнера во время нагрузки, не среднее потребление CPU и не раздельный расход памяти Nginx/Pingora/Axum.

Важно: искусственный Axum не обращается к PostgreSQL/S3 и не обрабатывает реальные права и пользователей. Результат отражает только стоимость проксирования для типовых размеров ответа. Не считать это доказательством ускорения TOR. После такого измерения нужна отдельная проверка реальных маршрутов с тестовой PostgreSQL, а перед выпуском — проверка всех правил Nginx, HTTP/2, заголовков, платного контента, отката и защиты от перегрузки. Никаких живых токенов или настоящих платных глав в стенде нет.

### Запуск

Автоматически при изменении experiments/pingora/** либо соответствующего файла GitHub Actions на PR. На другой машине вручную не запускать скрипт benchmark.sh: он отказывает вне GitHub Actions.

Исследование: https://app.notion.com/p/40732a84beec81d4b58ef133a4d2c2a2

## Дополнительный опыт: одинаковая интенсивность и причины расходов

Задача `fixed-rate-profile` отдельно измеряет Nginx и Pingora при 500, 1500 и 3000 запросов/с на 1 и 2 vCPU (по два повтора). Генератор на Go задаёт независимое время начала каждого запроса: если пропускная способность не успевает, записывает пропуски и ошибки, а не скрывает их. Для каждого случая сохраняются p50/p95/p99, фактическая пропускная способность, число новых соединений со стороны клиента, счётчик принятых TCP-соединений Axum, CPU-время, переключения контекста и RSS отдельных процессов Nginx/Pingora/Axum.

Файл `fixed-results/*.profile.json` содержит исходные разности счётчиков Linux `/proc`. Аппаратные циклы и распределение по функциям `perf` здесь не снимаются: GitHub-hosted runner может запрещать аппаратные счётчики. Их следует проверять отдельно на изолированной машине с `perf stat/record`, без изменения VPS. Не выводить «точную причину» из одних только `/proc` метрик. Если генератор не достигает заданной скорости, серия недействительна для сравнения равной нагрузки.

### Уточнение диагностики CPU/RAM и соединений

Исследование 29.10.2026 ([первый профиль](https://github.com/calex794/tor/actions/runs/40802407838)): 500/1500/3000 запросов/с при 1 и 2 vCPU, без потерь и ошибок; Pingora тратит примерно 1.5–1.6× CPU времени Nginx, а новые соединения с Axum создаёт существенно реже. Обе службы при таких уровнях дают p95 около 1.1–1.2 мс и имеют RSS порядка 12 МиБ. При этом измерение переключений контекста в первой версии считало только главный поток Rust — его нулевые значения НЕЛЬЗЯ трактовать как отсутствие переключений. Теперь process_snapshot.py суммирует их по всем потокам через /proc/PID/task/TID/status и помечает некорректную пару снимков при смене TID. Скрипт summarize_fixed.py выпускает summary.csv с медианой двух повторов и отбраковкой случаев, где генератор не достиг запрошенной частоты.

В этих измерениях нет аппаратного профиля CPU с распределением времени по функциям. Не заявлять точную причину, пока нет отдельных perf stat / perf record (если права тестовой машины позволяют).

## PINGORA-TOR-03 — optional CPU hot-spot sampling

An additional job, `perf-hotspots`, runs exclusively on a disposable Ubuntu GitHub Actions runner. It starts exactly the same loopback-only 1 vCPU / 1 GiB synthetic Axum, Nginx and Pingora, then supplies 1500 GET/s to each. `perf stat` attempts to capture cycles, instructions, CPU task-clock, cache misses and context switches for the proxy PIDs, separately. `perf record` then attempts to sample the most expensive CPU functions. The profiler adds overhead and its sampling run must NOT be used for a throughput comparison.

Some hosted runners deny hardware sampling or have a mismatched `perf` executable. In that case the job records a clear `PERF_UNAVAILABLE` / `PERF_PARTIAL` reason in the artifact and does not claim to have identified any hot function. The experiment does not change kernel security settings and never touches the production VPS. See `perf-results/availability.txt`, `*.stat.txt` and `*.top-symbols.txt`. Even with valid counters, compare only the same delivered 1500 requests/s and do not generalize to the live PostgreSQL-backed TOR.

### Hardware-counter caveat and improved CPU sampling (2026-10-29)

The first completed `perf` run [40857720133](https://github.com/calex794/tor/actions/runs/40857720133) counted 18,000 requests per proxy, with approximately 1.288 s Nginx task-clock and 2.271 s Pingora task-clock (1.76×). The Linux runner silently reported `<not supported>` for `cycles`, `instructions` and `cache-misses`, even though `perf stat` returned success. Thus its `PERF_AVAILABLE` label was incorrect. An absence of hardware counts does NOT establish the source of the extra CPU time. A preliminary perf sampling run had 29 Nginx and 74 Pingora samples, which is too small to reliably rank hot functions.

The probe now inspects event output and reports `PERF_PARTIAL` if hardware counters are unsupported; after each measurement it also checks stat results. To improve sampling coverage, the separate `perf record` pass explicitly samples software `task-clock` at 199 Hz for 24 s of fixed-rate requests (plus one second before/after). Sampling instrumentation alters performance; these results must never replace the uninstrumented latency/throughput tests. Data remains isolated to disposable GitHub Actions runners, with no changes to TOR production servers.
