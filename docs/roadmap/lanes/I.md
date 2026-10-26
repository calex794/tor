# I — Контракты, CI и выпуск: готовая инструкция TOR

**Дополнение 26.10 (PREF):** [OpenAPI 40 core routes](../contracts/openapi-v1.json), [API/CSRF/Idempotency](../contracts/api-evolution-v1.md), [61 F audit](functions-coverage-audit.tsv), [release flags and REL-01…10](../contracts/release-profile-v1.md). Задачи I-02/I-07/I-12 обязаны вести единые Rust typed ports, PG migrations и PG/S3/Chromium выпуск. `python3 tools/check_contracts.py` и архитектурные негативные проверки — mandatory в каждом PR.


**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7734/i-ci-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Уточнение аудита 26.10:** проверка 69 сценариев не гарантирует полноты 61 функций. Реестр [61 F](../functions-coverage-audit.tsv) и [аудит рисков](../preflight-audit-2026-10-26.md) обязательны для preflight, ADS/REC/ACL — typed контракты и отдельные PG/Chromium допуски. Нельзя подписывать 100% parity при выключенных платных рекламных системах или неподтверждённом внешнем wallet topup.

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** I меняет только общие миграции и wiring, бизнес-правила A–G не изменяет; принимает узкий контракт/DDL каждого владельца.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### I-01 — Д0

**Сделать:** Добавить source lock с фиксированными NEwBOO 17125ef758 и bokarust d11b27b431, версией TOR контрактов и реестром 69 S/61 F; валидатор должен обнаруживать смену без обновления ссылок.

**Главный выходной файл:** `docs/contracts/README.md` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** docs/contracts/README + матрица ссылок

**Сценарий и старый код:** [S059](https://app.notion.com/p/40432a84beec8132b372cf4b8f8ed791) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/antibot_service.py)), [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py)), [S062](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py))

**Принятый API/действие:** GET /metrics; trace correlation; CI deploy release/rollback; CI affected-test selection

**Проверка:** Нет путаницы с устаревшим 30-дневным графиком; исключения explicit. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `python3 tools/check_roadmap.py`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-02 — Д0

**Сделать:** Создать Cargo-пакет tor-contracts с положительными newtypes UserId/BookId/ChapterId/ChapterVersionId/PurchaseId, Actor/Permission, ErrorCode, интерфейсами ChapterRead/Entitlement и envelopes событий v1; добавить unit-тесты сериализации.

**Главный выходной файл:** `crates/contracts/src/lib.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** crates/contracts, docs/contracts; меняет только I

**Сценарий и старый код:** [S062](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py))

**Принятый API/действие:** CI affected-test selection

**Проверка:** Контракты компилируются и не содержат SQL/Axum; A–H получили версии. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `cargo check -p tor-contracts`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-03 — Д0–Д1

**Сделать:** Исправить и опробовать перенос bokarust: реальный Axum + PG в feature-first пакетах TOR.

**Главный выходной файл:** `crates/web/src/main.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** Cargo workspace, web main, migration release

**Сценарий и старый код:** [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** CI deploy release/rollback

**Проверка:** Первая коробка живёт в браузере; миграции с нуля и restart. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `cargo test -p tor-web --all-targets`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-04 — Д1 P0

**Сделать:** Воспроизвести browser-box PR #22: прямой GET закрытого draft HTTP 404 и отсутствие текста без смягчения ACL.

**Главный выходной файл:** `tests/browser/real_browser.test.cjs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** tests/browser + PG, CI workflow

**Сценарий и старый код:** [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Полный путь register → publish → read → progress → deny, зеленый SHA. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `node --test tests/browser/real_browser.test.cjs`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-05 — Д1

**Сделать:** Поднять PG service в Actions, SQLx миграции, smoke/readiness, анонимная и authenticated сборка.

**Главный выходной файл:** `.github/workflows/postgres-e2e.yml` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** workflows/Rust, migrations/*

**Сценарий и старый код:** [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** CI deploy release/rollback

**Проверка:** CI настоящей БД и HTML; /healthz не объявляется DB readiness без проверки. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `cargo test --workspace --all-targets`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-06 — Д1–Д2

**Сделать:** Закрепить registry модулей и владельцев, feature-first проверки, branch protection/manual merge gate.

**Главный выходной файл:** `architecture/modules.toml` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** architecture/modules.toml, GitHub Settings

**Сценарий и старый код:** [S062](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py))

**Принятый API/действие:** CI affected-test selection

**Проверка:** Существующий guard проверен; настройки branch-protection требуют владельца с правом администратора. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `python3 tools/check_feature_layout.py`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-07 — Д2

**Сделать:** Создать CI матрицу cheap/feature/PG/browser/full и критерий запуска из changed paths + dependencies.

**Главный выходной файл:** `tools/select_feature_tests.py` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** tools/select_affected_tests, workflows

**Сценарий и старый код:** [S062](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py))

**Принятый API/действие:** CI affected-test selection

**Проверка:** Общий контракт/неизвестный файл → широкий safe-run; изменения leaf не поднимают все unrelated tests. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `python3 -m unittest discover -s tests/architecture -v`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-08 — Д2–Д5

**Сделать:** Принимать PR A–H по актуальному main, обрабатывать конфликт migration/router и перепроверять composite SHA.

**Главный выходной файл:** `crates/web/src/lib.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** Integration queue, web routing

**Сценарий и старый код:** [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** CI deploy release/rollback

**Проверка:** Нет двух зелёных PR на старом SHA без совместной проверки. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `cargo test --workspace --all-targets`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-09 — Д3

**Сделать:** Создать tools/ci_metrics.py: по GitHub Actions run выводить число запущенных jobs, p50/p95 очереди/прогона и суммарные runner-minutes для сравнения пакетной ветки с обычной; отчёт привязать к SHA.

**Главный выходной файл:** `tools/ci_metrics.py` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** run analytics report

**Сценарий и старый код:** [S062](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py))

**Принятый API/действие:** CI affected-test selection

**Проверка:** Не уменьшать необходимое покрытие ради зелёного статуса; фактические цифры сравнить до/после. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `python3 tools/ci_metrics.py --report`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-10 — Д3–Д4

**Сделать:** Настроить выделенный staging хост, TLS, конфиг, отдельную PostgreSQL, secrets, readiness и журнал.

**Главный выходной файл:** `docs/deployment.md` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** deploy plan, secrets not in repo

**Сценарий и старый код:** [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** CI deploy release/rollback

**Проверка:** На малом VPS данные не хранятся только в RAM; нет пользовательских денег без PG. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `curl --fail "$TOR_STAGE_URL/readyz"`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-11 — Д4

**Сделать:** Версионированный release, smoke/checkpoint, переключение и быстрый rollback; backup/restore.

**Главный выходной файл:** `deploy/release.sh` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** deploy scripts, migration policy

**Сценарий и старый код:** [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** CI deploy release/rollback

**Проверка:** Падение новой версии сохраняет предыдущую рабочую; rollback БД спроектирован. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `bash deploy/release.sh --dry-run`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-12 — Д5

**Сделать:** Собрать интеграционный кандидат: все разрешённые функции, фиксированный SHA, матрица разрывов.

**Главный выходной файл:** `docs/roadmap/release-checklist.md` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** release candidate, Notion parity

**Сценарий и старый код:** [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** CI deploy release/rollback

**Проверка:** Нет незадокументированного блокера P0, критичные ACL/ledger тесты зелёные. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `cargo test --workspace --all-targets`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### I-13 — Д5

**Сделать:** Добавить автоматические выпускные проверки S059 tracing (request_id без PII), S061 версионированного deploy/rollback и S062 affected-test selector; прикрепить реальный CI-run и матрицу результатов к текущим карточкам Notion.

**Главный выходной файл:** `docs/roadmap/scenarios.tsv` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** Notion matrix + CI evidence

**Сценарий и старый код:** [S059](https://app.notion.com/p/40432a84beec8132b372cf4b8f8ed791) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/antibot_service.py)), [S061](https://app.notion.com/p/40432a84beec816aa599eab58fd823c6) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py)), [S062](https://app.notion.com/p/40432a84beec81a18573cc73a6db3ece) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/scripts/test_impact.py))

**Принятый API/действие:** GET /metrics; trace correlation; CI deploy release/rollback; CI affected-test selection

**Проверка:** Каждому статусу есть лог/trace/скрин/SQL и исходный эталон. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Команда контрольной проверки (инструменты создаются по соответствующему шагу):** `python3 tools/check_roadmap.py`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## SEM — смысловая карта маршрутов, 26.10.2026

[I-07/I-12: карта кандидатных действий](../legacy-inventory/route-action-candidates.tsv) обеспечивает **568 записей**, 420 групповых кандидатных соответствий F001–F061, 45 дополнительных действий, 101 frozen, 1 disabled, 1 не подключённое в фасаде. Это только **трассировка**, не 420 подтверждённых функций: CI `tools/check_legacy_coverage.py` проверяет соответствие каждого старого маршрута этой карте и реестру 117 задач/61 F, запрещает ложную пометку «проверено». `--strict` по-прежнему красный до семантических и PG/HTTP/Chromium тестов. [AUTHOR-01…08](../author-workspace-execution-contract.md) включить в B-14/F-12/H-07/I-12, без восстановления замороженного URL crawler.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
