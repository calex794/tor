# G — Импорт и фоновые события: готовая инструкция TOR

**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7732/g-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Уточнение аудита 26.10:** обновление рекомендательных generations и ad serving требует durable jobs и bounded delivery ([REC](../recommendations-execution-contract.md), [ADS](../ads-execution-contract.md)); G не выдаёт права, не списывает деньги, не меняет authoritative book content. Парсеры проверяются по реальному allowlist и сохранённым HTML fixtures.

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** I outbox contract, B authoritative book commands, D purchase events, E subscriptions, F projections; G не пишет чужие таблицы.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### G-01 — Д0–Д1

**Сделать:** Создать типы SourceIdentity, ImportJob, FileObject, EventId, lease_epoch и валидаторы принятых ограничений: image ≤8 МиБ/24 Мп, архив ≤32 МиБ/256 МиБ распаковки/5000 файлов/ratio≤50, https-only allowlist.

**Главный выходной файл:** `crates/ingest/src/domain/file_limits.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** ingest/domain, contracts I

**Сценарий и старый код:** [S023](https://app.notion.com/p/40432a84beec817385d5fcd73e3d46f1) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py)), [S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapter_import.py)), [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py))

**Принятый API/действие:** POST /api/v1/import-jobs; POST /api/v1/import-jobs/{id}/upload; POST /api/v1/import-jobs (mode=url); POST /api/v1/books/{id}/cover

**Проверка:** Не утверждать неизвестный лимит обложки/архива; тесты на границы после согласования. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-02 — Д1 P0

**Сделать:** Импорт набора глав из файла, предварительный просмотр/статус/транзакционное прикрепление.

**Главный выходной файл:** `crates/imports/src/services/import_file.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** ingest/api,services, repositories/jobs

**Сценарий и старый код:** [S023](https://app.notion.com/p/40432a84beec817385d5fcd73e3d46f1) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py))

**Принятый API/действие:** POST /api/v1/import-jobs; POST /api/v1/import-jobs/{id}/upload

**Проверка:** Сбой на главе №N не оставляет половинной незаявленной публикации. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-imports --test slices_pg g_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-03 — Д2 P1

**Сделать:** Импорт с URL: каталог/страницы, извлечение title/chapter, абсолютная identity, безопасные host allowlists.

**Главный выходной файл:** `crates/imports/src/services/import_url.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** imports/fetch/parse, source_identities

**Сценарий и старый код:** [S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapter_import.py))

**Принятый API/действие:** POST /api/v1/import-jobs (mode=url)

**Проверка:** SSRF/localhost/file:// и запрещённые сети недоступны; один source не создаёт дубль. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-imports --test slices_pg g_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-04 — Д2 P1

**Сделать:** Управление import jobs: очередь, этапы, пакетирование, pause/resume/cancel, checkpoint.

**Главный выходной файл:** `crates/imports/src/services/job_control.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** import_jobs,job_checkpoints

**Сценарий и старый код:** [S023](https://app.notion.com/p/40432a84beec817385d5fcd73e3d46f1) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py)), [S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapter_import.py))

**Принятый API/действие:** POST /api/v1/import-jobs; POST /api/v1/import-jobs/{id}/upload; POST /api/v1/import-jobs (mode=url)

**Проверка:** После рестарта продолжает с checkpoint без пропусков и дублей. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-imports --test slices_pg g_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-05 — Д2–Д3 P1

**Сделать:** Обложка и изображения: MIME/sig, размеры, декодирование без доверия к расширению.

**Главный выходной файл:** `crates/ingest/src/services/images.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** media/storage, file_previews

**Сценарий и старый код:** [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py))

**Принятый API/действие:** POST /api/v1/books/{id}/cover

**Проверка:** Полиглот/сверхбольшое изображение/архив-zip-bomb отвергнуты. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-06 — Д3 P0

**Сделать:** Outbox для B publish, D purchase, E notifications, F projections; envelope schema/version.

**Главный выходной файл:** `crates/ingest/src/repositories/outbox.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** outbox_events/delivery_attempts

**Сценарий и старый код:** [S066](https://app.notion.com/p/40432a84beec817cac8be9193efed60d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** POST /api/v1/admin/jobs/{id}/retry

**Проверка:** Commit publish без доставки не теряет событие; повтор dedupe. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-07 — Д3–Д4 P1

**Сделать:** PostgreSQL durable job claims с lease/fence, heartbeat/retry/backoff/dead letter.

**Главный выходной файл:** `crates/ingest/src/services/workers.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** jobs/runner, job_leases

**Сценарий и старый код:** [S066](https://app.notion.com/p/40432a84beec817cac8be9193efed60d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** POST /api/v1/admin/jobs/{id}/retry

**Проверка:** Просроченный worker не отмечает успех и не подавляет нового владельца. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-08 — Д4 P1

**Сделать:** Уведомление подписчиков: batches, cursor, preferences/opt-out, не терять 201-го.

**Главный выходной файл:** `crates/ingest/src/services/notify.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** notifications/read models, recipient_checkpoint

**Сценарий и старый код:** [S066](https://app.notion.com/p/40432a84beec817cac8be9193efed60d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** POST /api/v1/admin/jobs/{id}/retry

**Проверка:** 201+ адресатам доставляется по частям без повторов. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-09 — Д4 P1

**Сделать:** Файловая долговечность: provisional attach, object hash, cleanup, storage недоступно.

**Главный выходной файл:** `crates/ingest/src/services/storage.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** file_objects, storage_adapter

**Сценарий и старый код:** [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py))

**Принятый API/действие:** POST /api/v1/books/{id}/cover

**Проверка:** Битый объект не прикрепляется к книге; недоступный S3 не нарушает PG. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-10 — Д5 P2

**Сделать:** Материализация read models, SEO/recommendation jobs, retry/invalidation и read-only monitoring.

**Главный выходной файл:** `crates/ingest/src/services/projections.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** projection_queue, job_status

**Сценарий и старый код:** [S066](https://app.notion.com/p/40432a84beec817cac8be9193efed60d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** POST /api/v1/admin/jobs/{id}/retry

**Проверка:** Повтор события не создаёт конкурирующую authoritative модель. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-11 — Д5 P0

**Сделать:** Интеграционный импорт: URL/file → preview → B командой создать → publish → F индекс → E notify.

**Главный выходной файл:** `crates/ingest/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** I wiring + G tests

**Сценарий и старый код:** [S023](https://app.notion.com/p/40432a84beec817385d5fcd73e3d46f1) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py)), [S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapter_import.py)), [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S066](https://app.notion.com/p/40432a84beec817cac8be9193efed60d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** POST /api/v1/import-jobs; POST /api/v1/import-jobs/{id}/upload; POST /api/v1/import-jobs (mode=url); POST /api/v1/books/{id}/cover; POST /api/v1/admin/jobs/{id}/retry

**Проверка:** Публичная книга появляется только после успешного B commit. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### G-12 — Д5

**Сделать:** Написать интеграционные PG-тесты импорта S023–S025, retry S066 и технической доставки уведомлений E/S018; предъявить каждому положительный/отрицательный исход, сохранённый checkpoint и отсутствие дублей после restart.

**Главный выходной файл:** `crates/ingest/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** Notion parity

**Сценарий и старый код:** [S023](https://app.notion.com/p/40432a84beec817385d5fcd73e3d46f1) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py)), [S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapter_import.py)), [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S066](https://app.notion.com/p/40432a84beec817cac8be9193efed60d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/db/clients/async_ydb_client.py))

**Принятый API/действие:** POST /api/v1/import-jobs; POST /api/v1/import-jobs/{id}/upload; POST /api/v1/import-jobs (mode=url); POST /api/v1/books/{id}/cover; POST /api/v1/admin/jobs/{id}/retry

**Проверка:** Все внешние источники/лимиты/непроверенные сайты отмечены. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-ingest --test slices_pg g_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
