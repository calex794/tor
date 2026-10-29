# I-07 / S062 — выбор существующих проверок TOR

`tools/select_feature_tests.py` выбирает **существующие workflow целиком**.
Команды Rust, PostgreSQL, S3, Chromium и нагрузочного теста не сокращены.
`Repository rules` выполняется всегда. Отбор отдельных Rust test functions и
индекс наблюдаемых вызовов Python `test_impact.py` в этот срез не входят.

Источник поведения: [NEwBOO test_impact.py на 17125ef758](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py).
Контракт TOR: [I-07](../roadmap/lanes/I.md),
[S062 в существующем реестре](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece),
[обязательные связанные испытания](../roadmap/tests-and-handoffs.md).

## Когда выполняется полный набор

- Каждый `push` в `main` и каждый ручной `workflow_dispatch` выполняют все
  14 существующих наборов, включая PostgreSQL 18, оба S3-провайдера,
  Rust/WebCrypto, четыре браузерных набора, PostgreSQL 100k и восстановление
  зашифрованных резервных копий и купленной V1.
- Общие Cargo-файлы, SQL, контракты, CI, ACL, деньги, публикация и доставка
  выбирают полный набор. Неизвестный путь, новая предметная область,
  отсутствующая история, пустой diff или ошибка чтения зависимостей — тоже.
- Ошибка самого задания `select` не блокирует запуск тестов через обычное
  поведение `needs`: условие каждого задания явно запускает его при сбое
  селектора. Отмена workflow остаётся отменой, а не успешной проверкой.

Деплой `tor-vps-staging.yml` и `tor-one-click-release.yml` в селектор не включён
и остаётся только ручным, с собственными проверками выбранного SHA.
Полный CI не запускает выкладку.

## Как выбираются проверки PR

1. Checkout проверяемого коммита с полной историей, только `contents: read`,
   без сохранения credentials и без `pull_request_target`.
2. Локальный `git diff BASE...HEAD --no-renames --name-only -z`.
   Нет ограничения GitHub path filters в 300 файлов. Переименование учитывается
   как удалённый старый и добавленный новый путь; имена с пробелами и переводами
   строк не превращаются в команды оболочки.
3. `tools/ci_test_map.py` хранит входные Cargo-пакеты каждого набора и точные
   пути отдельных тестов/браузерных скриптов. Для исходников учитываются
   транзитивные локальные Cargo dependencies, включая aliases,
   workspace inheritance, optional, build/dev и target-specific зависимости.
   Optional/target зависимости берутся консервативно все.
4. Обязательные критические области имеют приоритет над графом. Только
   явно перечисленные текстовые документы могут дать режим `cheap`.
5. Каждый workflow всегда создаётся на PR/main. Условие стоит на задании,
   поэтому прежние имена workflow и тестовых заданий сохранены, а path filters
   больше не скрывают обязательный статус целиком.

| Изменение PR | Выбранные проверки сверх Repository rules |
| --- | --- |
| `README.md` | Нет тяжёлых заданий |
| `crates/ingest/tests/image_pipeline.rs` | Rust и image pipeline |
| `tools/visual-smoke.cjs` | Visual smoke |
| `tools/paid_reader_minio_chromium_e2e.cjs` | Paid S3 grant E2E с настоящим Chromium |
| `tools/paid_grant_webcrypto_e2e.mjs` | Paid S3 grant E2E и восстановление купленной V1 |
| `tools/tor_dr_archive_crypto.cjs` | Оба набора восстановления, использующие общий helper |
| `crates/recommendations/src/services/ranking.rs` | Rust, PG и все потребители web, включая paid E2E и оба набора восстановления |
| `crates/contracts/src/paid_delivery.rs`, миграция, `Cargo.lock`, неизвестный файл | Все 14 наборов |

Для каждого запуска сохраняется JSON `tor-ci-selection-<suite>`: SHA базы и
проверяемого коммита, все изменённые пути, причины, затронутые Cargo-пакеты и
булева матрица. Это решение о запуске, **не** отчёт об успешных тестах.
Пока каждый workflow вызывает лёгкий планировщик отдельно. Для I-09 уже
получен [фактический замер](metrics-i09.md) двух SHA `main` с одинаковыми
29 исполненными заданиями. Устойчивое ускорение или снижение стоимости
**не доказано**: изменялись код и загрузка исполнителей. Программа I-09
запрещает сравнение, если хоть одно задание пропущено, изменён его состав
или закончилась неуспехом проверка. Наличие обязательных наборов также
не доказывает ещё не реализованный сквозной платный сценарий.

## Локальная проверка и расширение

```bash
python3 tools/select_feature_tests.py --path crates/ingest/tests/image_pipeline.rs
python3 tools/select_feature_tests.py --base FULL_BASE_SHA --head FULL_HEAD_SHA
python3 tools/select_feature_tests.py --full
python3 -m unittest discover -s tests/architecture -v
```

Изменение реестра, селектора или workflow само выбирает полный набор.
Для нового набора добавить входные Cargo-пакеты и workflow в `SUITES`, вызвать
`ci-selection.yml` и сохранить условие с запуском при ошибке planner. Для нового
теста сначала проверить, какая реальная команда его исполняет; неизвестный тест
не добавлять в узкое исключение по одному имени. Архитектурные проверки
контролируют подключение всех workflow и негативные случаи выбора.
