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
