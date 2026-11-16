# TOR — единые контракты (решения приняты; код переносится по роадмапу)

**REC/FLOW v2 P2a:** [остановка индивидуального runtime и граница P2b](flow-rec-runtime-retirement-v2.md). Новый сервер не возобновляет сбор по старым флагам; отдельные согласия удаляются из пользовательского пути. Это не утверждение полного SQL/RAM cutover или завершения этапов FLOW.

Исходная спецификация описывала вопросы, которые исполнитель должен был «проработать». Эти решения **уже записаны** в исполняемой версии:

1. **[Архитектура, ID, роли и совместимость](../roadmap/decisions-v1.md):** единый Actor, предметные Cargo-пакеты, source SHA, отказ при неизвестном праве, 43-символьные idempotency keys, UTC, недопустимость фиктивных платежей.
2. **[HTTP API v1](../roadmap/api-v1.md):** конкретные URL/методы, формы/JSON, статус ошибки, доступ, пределы и запрет смены формы уже действующего `GET /api/v1/books`.
3. **[PostgreSQL, события, лимиты](../roadmap/data-and-events.md):** миграции 001–009 из проверенной Rust основы, новая paid-progress 010, outbox 011, владельцы таблиц, отказные случаи и утверждённые технические размеры файлов.
4. **[Исполнительские девять направлений и 117 задач](../roadmap/README.md):** файлы, входы, проверки, исходники, расписание 5+7+3.
5. **[Инструкция по CI, фикстурам и PR](../roadmap/tests-and-handoffs.md):** дешёвая проверка → законченный предметный блок → пакетное устранение ошибок → обязательная финальная регрессия на интеграционном SHA.

**REC/FLOW v2 P1 — явный выбор читателя:** [rec-explicit-preferences-v1.md](rec-explicit-preferences-v1.md) переносит скрытия книг и квитанции команд в независимое хранилище аккаунта. Это подготовка смены модели; согласия и guards индивидуального сбора пока не удаляются, завершение FLOW не заявлено.

**FLOW-02 — ограниченные почасовые агрегаты:** [flow-visit-rollups-v1.md](flow-visit-rollups-v1.md) описывает HTML-запросы и наблюдаемые согласованные визиты поверх RIL. **Не число уникальных посетителей и не готовый dashboard.**

**FLOW-01 — измерение аудитории:** [flow-analytics-v1.md](flow-analytics-v1.md) описывает два канала атрибуции, семантику визитов и доверенных фактов. Исполняемый контракт — `tor-contracts::flow`, тесты `flow_contract.rs`. **Это ещё не сбор визитов и не dashboard.**

Новые типы `tor-contracts` и обработчики **ещё требуется реализовать** по принятым контрактам. Документы — подготовка, а не доказательство выполняемых API. Не создавать параллельный `new_contracts` / вторую систему прав из-за неполноты текущего `tor/main`.

**Внешние границы:** S041 Boosty, S003 OAuth и рекламная активация работают только после реальных подписанных конфигураций и end-to-end теста. Замороженные S053–S056 не реализовывать без нового продуктового решения.

Оригинал продукта: [Notion v0.2](https://app.notion.com/p/40432a84beec81c49bbad75e88ff6989). Истинный статус функций: [Notion](https://app.notion.com/p/970f1eab10e041489e285725a697b513).

- [FLOW observation-hour counts v1](flow-observation-hours-v1.md): anonymous RIL event-hour projection, periods/timezones and evidence boundaries.

- [FLOW-05 explicit library/follow snapshots](flow-explicit-choices-v1.md): current product choices, surviving cohorts, not historical additions or conversion.

- [FLOW-05 canonical book reading](flow-canonical-book-reading-v1.md): retained C first-completion pairs, current choices and honest scope.

- [REC contextual metadata/manual model v1](rec-contextual-model-v1.md): FLOW/REC P2b2 versioned builds and explicit activation; no individual collection.

- [FLOW-07 external search snapshots](flow-search-analytics-v1.md): fixed-provider imports, durable leases, independent totals, coverage and admin reports; provider activation remains explicit.

- [FLOW-05 anonymous card observations](flow-anonymous-observations-v1.md): trusted per-card proofs, browser viewability, explicit coverage and low observed exposure; no reader identity or billable impressions.

- [S349 free S3 delivery](free-s3-delivery-v1.md): bodyless new free editions, verified publication, direct browser reading and immutable source positions.

- [DROP Free S3 Single/Pack placement v1](tor-store-free-placement-v1.md): non-activating exact-edition read planner; private Range Gateway required for PACK, no signed whole-pack URLs or live migration.
- [DROP-R2 PostgreSQL placement generations](tor-store-free-placement-r2-pg.md): additive inactive free PACK catalog, immutable generations, exact edition CAS and disposable PG18 checks; no production activation.
- [DROP-R3a private PACK Range reader](tor-store-r3a-private-pack-range.md): inert bounded Zstd frame decoder, internal S3 Range adapter, two authorization checks, real private MinIO tests; no HTTP activation.
- [DROP-R3b full PACK proof](tor-store-r3b-full-pack-proof.md): immutable full-object and manifest GET/sha256, exact seekable table, all editions decoded and matched against B identities; no active reader route.
- [DROP-R4a lease-fenced PACK attachment](tor-store-r4a-fenced-attachment.md): B-owned all-or-nothing PG candidate generations after G verified proof, current lease and approved fingerprint; no active reader cutover.

- [DROP-R4b1 trusted offline PACK readback](tor-store-r4b1-trusted-worker.md): immutable B job specs and epoch claims, G independent full S3 GET before B PG candidate attachment; no upload worker, reader cutover or public route.

- [DROP-R4b2 immutable PACK writer](tor-store-r4b2-immutable-writer.md): bounded standard Zstd seekable builder, private conditional S3 PUT and independent full readback before B fenced attachment; no live reader activation.
