# TOR HTTP v1 — выполнение OpenAPI, ошибок, сериализации и совместимости (PREF-07)

**Машинно-читаемое ядро:** [openapi-v1.json](openapi-v1.json) (OpenAPI 3.1.0, 40 HTTP-операций). **Подробные канонические соглашения:** [api-v1.md](../roadmap/api-v1.md), [paid S3](paid-delivery-v1.md), [права](../roadmap/access-authorization-contract.md), [реклама](finance-ad-exact-v1.md), [возвраты](refunds-payouts-v1.md), [файлы](import-storage-v1.md), [сигналы](events-analytics-v1.md). Это CONTRACT документации; Rust реализацию/SQLx-модели требуется создать и доказать, OpenAPI файл **не запускает сервер**.

## 1. Где живёт код и кто может изменять общий договор

- **I:** `docs/contracts/openapi-v1.json`, `crates/contracts` typed schemas+error enum+serde, `crates/web` composition/root route, единственные shared migrations. I разрешает изменение contract через PR после affected-domain reviews и changelog `api_version`.
- **A–H:** каждая функция владеет только `crates/<domain>/src/api/services/repositories/domain`, **не** добавляет второй Axum server/web root, не пишет SQL других доменов. API adapter вызывает typed domain service; SQLx только в собственных репозиториях. Cross-domain порты/факты — `crates/contracts`.
- **H:** HTML формы сохраняют действующие `/register, /login, /logout, /catalog, /books/{id}, /reader/{id}, /chapters/{id}/buy` и используют **те же use cases**, не собственный wallet/auth. Историческое поле HTML `command_key` транслируется в типизированный `IdempotencyKey` на сервисной границе; JSON использует **только заголовок**.

## 2. Wire invariants

| Поле | Договор |
| --- | --- |
| ID | JSON positive decimal **string**, не JS number; old GET `/api/v1/books` исторически возвращает массив, не менять в первом релизе |
| Money | `amount_minor/price_minor/budget_minor` integer number только RUB, минимум 0 или 1 в зависимости от команды, без floating-point/decimal money; ad-модель [FIN-01](finance-ad-exact-v1.md) |
| Time | UTC RFC3339 `2026-10-26T00:00:00Z`; PG TIMESTAMPTZ, не local naive |
| Body | `application/json` со строгой схемой: unknown field → 422 `UNKNOWN_FIELD`; default≤64 КиБ, лимиты domain в `data-and-events.md` |
| Replay | Для mutating POST с ledger/grant/созданием: header `Idempotency-Key` = 43 chars base64url без padding из **32 случайных байтов**. Scope=(actor,route,command_kind); exact canonical payload same receipt, changed payload→409. Не засчитывать простой retry S3 signed GET как новую purchase. |
| Cookie security | `tor_session`, Secure+HttpOnly+SameSite; все cookie-mutating POST/PATCH/PUT/DELETE проверяют Origin и `X-CSRF-Token`; **HTML form** может присылать `csrf_token` field, JSON body его не дублирует. CSRF/Origin failure→403, rate→429. |
| Error | только `{"error":{"code":"...","message":"...","request_id":"..."}}`, no internals; 401 no session, 403 authenticated denial, 404 hidden, 409 revision/replay/concurrency, 413 size, 422 invalid domain, 429 rate, 503 provider outage |
| Cursor | opaque signed/versioned cursor max 512 bytes; page limit 1..100, default20, keyset order+filter included in token; tampered/expired cursor→400 |
| Cache | private reader/grant/wallet/statement: `private,no-store`; public catalog caches invalidate immediately on moderation/publication; paid plaintext/URL/DEK NEVER shared |
| Visibility | all query response DTOs apply A/B `CanDiscover` before return, including search/recs/ads/SEO and CDN. No hidden text snippets or archived asset names in errors. |

**Сервисные идентификаторы:** клиент не присылает `actor_id`, `wallet_id` или `publisher_balance` в команды как право; сервер берёт actor из session, остальное по indexed authority. Если запрос содержит неизвестный `actor_id` вне документированного DTO — 422. Никакой прямой HTTP handler не пишет money/entitlement вне D.

## 3. Изменения против самого раннего исторического draft

- **Платная глава**: `GET /api/v1/chapters/{id}/read` = только meta+`grant_required`; `POST /api/v1/reader/chapters/{id}/delivery-grants` = 90-секундный scoped signed private S3 ciphertext+per-version DEK. НЕ `body_markdown` из Rust, никаких plaintext через reverse proxy. Хранение полное по [S3 v1](paid-delivery-v1.md).
- **Прогресс**: `PUT /api/v1/me/progress/{free|purchased}` отправляет `block_id` из B ReaderVersionManifest + `char_offset` и expected_revision, а не произвольный массив client block_index; только server-trusted final block creates ChapterCompleted.
- **Реклама**: прежний общий `/api/v1/ads/campaigns` не является единственным контрактом. Есть **отдельные** `/api/v1/ads/daily-campaigns` (legacy fully prepaid days) и `/api/v1/ads/chapter/campaigns` + `/packages`/`impressions` (verified viewable package). Внутренняя реклама **не** требует внешней рекламной сети, но реальные charging/settlement включаются только после D PG tests. Рекламный client viewability report сам по себе не trusted.
- **Импорт**: `POST /api/v1/import-jobs` принимает только `mode="upload"`; `source_url/mode=url` — 422 `URL_IMPORT_FROZEN`. Не создавать route, crawler job, URL validator/test. [Notion S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) — владелец заморозил без срока.
- **Платёж через Boosty**: `503 PROVIDER_NOT_CONFIGURED` без доказанного external receipt/contract. Тестовое пополнение через SQL fixture не открывается публичным маршрутом.
- **Выплаты**: `POST /me/payout-requests` возвращает `pending/reserved` и никогда `posted` обычному читателю. Переход к реально выплачено возможен только по внешнему evidence.
- **Дефолты флагов**: согласно [launch profile](release-profile-v1.md), незавершённые платные операции выключены, при этом запрещено возвращать fake 200 success.

## 4. Набор обязательных контрактных проверок до кодирования доменов

I создаёт `crates/contracts/src/{auth,books,reading,commerce,ads,recommendations,imports,events,errors}.rs`, сериализует DTO одинаково в Axum и тестах. Список в OpenAPI — минимально достаточные P0/P1 **40 операций**, не утверждение, что каждый вспомогательный маршрут социальных функций из текста уже покрыт машинной схемой. Неизвестные вспомогательные API до опубликования schema остаются не монтированы и не считаются готовыми.

| Проверка | Что должно упасть |
| --- | --- |
| API-01 | исчез S3 delivery-grant, вернулся `body_markdown` в reader schema |
| API-02 | присутствует `/api/v1/import-jobs?mode=url`, route `/crawler` или `source_url` в ImportCreate |
| API-03 | `Idempotency-Key` продублирован в JSON body или не требует 43 символа |
| API-04 | деньги в `f64`/decimal string, `actor_id` в request purchase |
| API-05 | в state-changing cookie route отсутствует Origin/CSRF |
| API-06 | две одинаковые purchase команды дали два ID/debit, иной payload не дал 409 |
| API-07 | paid V1 после V2 выдаёт произвольный `version_id` клиента |
| API-08 | POST реального topup/paid ads с выключенным feature flag возвращает 200 |
| API-09 | JSON unknown fields silently ignored, expired cursor принялся |
| API-10 | private/grant/DEK leaked in HTML, traces, cache или open API |
| API-11 | schema объявлена и документ есть, а соответствующий handler ещё не реализован; статус «готово» запрещён |
| API-12 | OpenAPI объявил typed source, а его feature owner не добавил собственный PG/browser endpoint test |

**CI:** `python3 tools/check_contracts.py` валидирует OpenAPI refs, required high-risk paths и отсутствие URL-import; negative tests в `tests/architecture/test_contracts.py` проверяют, что нарушение вызывает fail. После фактического Rust кода: contract-driven integration with PG, 2 Actors, real private S3 and Chromium on one SHA. Более широкий API по E/H требует отдельного расширения OpenAPI с соответствующим PR; нельзя объявлять полный v1 контракт завершённым только числом операций.

**Изменения без инвалидирования клиентов:** допускается добавить необязательное response поле с `api_version` и тестом; сужение auth, удаление поля, изменение цены/единицы денег или error status — breaking, требует новой major версии/dual old compatibility и явного документа миграции. Не отдавать SQL row в публичном JSON.
