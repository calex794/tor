# PINGORA-TOR-01 — изолированный опыт

Это самостоятельный Cargo-проект вне рабочего TOR. Рабочий Nginx, RIL, база и VPS не изменяются.

Nginx уже принимает HTTPS и фильтрует запросы. RIL классифицирует маршруты и измеряет стоимость. Pingora **не повторяет** эти функции: здесь нет кеша, ограничителей или решений о правах.

## Запуск на отдельной машине

Нужны Rust stable, pkg-config, OpenSSL headers и тестовый HTTP-источник на 127.0.0.1:6194.

~~~sh
cd experiments/pingora
cargo generate-lockfile
cargo test --locked
PINGORA_EXPERIMENT=1 PINGORA_EXPERIMENT_BIND=127.0.0.1:6193 PINGORA_EXPERIMENT_UPSTREAM=127.0.0.1:6194 cargo run --locked
~~~

Опыт не запускается без PINGORA_EXPERIMENT=1 и отказывается принимать публичные адреса или внешние источники.

## Как сравнивать

Сравнить Nginx → тестовый Axum, Pingora → тот же Axum и Nginx → Pingora → Axum на отдельном тестовом окружении. Сначала проверить 200, 404, 429, 503, затем заголовки Host, Connection, X-Real-IP, X-Forwarded-For, ошибочный upstream и медленного клиента. Никаких реальных персональных данных, платежей и токенов.

Проводить не менее пяти повторов с одинаковым порядком чередования, прогревом, HTTP/1.1 keep-alive и ограничением 1 vCPU / около 1 GiB RAM. Измерять p50/p95/p99, запросы/сек, ошибки, RSS и CPU. Не считать фиктивный HTTP-источник показателем реального ускорения TOR.

~~~sh
wrk -t1 -c16 -d30s --latency -s experiments/pingora/wrk_metrics.lua http://127.0.0.1:6193/
~~~

Скрипт выдаёт TOR_PINGORA_METRIC. Отдельно после базового теста сравнить на тестовой PostgreSQL и основных маршрутах TOR. При отсутствии доказанного преимущества оставить Nginx + RIL. Положительный тест не разрешает выкладку на VPS.

Исследование: https://app.notion.com/p/40732a84beec81d4b58ef133a4d2c2a2
