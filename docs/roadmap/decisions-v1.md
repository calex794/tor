# TOR — принятые решения для исполнителя (календарь 26.10–09.11.2026)

**Статус:** нормативная рабочая спецификация для чистового `calex794/tor`. Здесь собраны решения, которые до этого были сформулированы как «определить/согласовать/проработать». **Не** считать новые API и таблицы уже существующими. У задачи есть готовое выбранное поведение — исполнителю остаётся реализовать и проверить, а не проектировать. Внешние возможности без работающего проверенного провайдера выключены, а не имитируют успех.

**Эталон поведения:** [NEwBOO `17125ef758`](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228); **донор рабочего Rust/PG:** [bokarust `d11b27b431`](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b). **Цель:** TOR. [Функции F001–F061](https://app.notion.com/p/6afdef6d407c4be6b7f184a49d0408f3), [сценарии S001–S069](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d).

## 1. Непересматриваемый способ реализации

- Один Git-репозиторий `tor`, один процесс `tor-web` на Axum для сайта/API; PostgreSQL — источник состояния. Фоновые исполнители могут быть отдельным процессом на общей базе, но не отдельными сетевыми микросервисами.
- Код организован **по предметным Cargo-пакетам**, а внутри по слоям `src/api/` → `src/services/` → `src/repositories/`, отдельно `src/schemas/` и `src/domain/`. Примеры: `crates/accounts`, `crates/books`, `crates/reading`, `crates/commerce`. Никаких общих каталогов с сотнями services/repositories.
- Точное правило зависимостей: feature-crate → `tor-contracts`; `tor-web` связывает компоненты. **Нет** прямых Cargo-зависимостей feature→feature; запрещено читать и изменять таблицы чужого владельца ради принятия бизнес-решения. Узкие типизированные порты объявлять в `contracts`, реализации отдавать из своего пакета. Отсутствие библиотеки `contracts` сейчас — задача I-02, а не повод создавать местные дубли типов.
- У каждого набора данных и каждой операции записи один владелец. Общие `Cargo.toml`, `architecture/modules.toml`, `migrations/`, `contracts/`, `crates/web/src/main.rs` и общий роутер меняет **линия I**; A–H передают ей подготовленный контракт/заявку. Линия H не владеет бизнес-таблицами, а делает страницы по операциям A–G.
- Финансовая операция — один PostgreSQL commit для денег, журнала и права. Опубликованная версия текста — неизменяема. Наличие книги в библиотеке/прогресса никогда не является разрешением читать платный текст.
- Каждая задача выполняется **вертикально**: доменное правило → служба → PostgreSQL-репозиторий → HTTP → тест → сопряжение, а не сначала «все схемы всех модулей» и потом первый работающий сайт.

## 2. Как исполнитель создаёт новый модуль

Из `crates/books` взять layout и имя пакета, но не переносить его S0 in-memory адаптер в production. Для, например, `reading` создать:
```text
crates/reading/Cargo.toml              # package tor-reading, lib entry
crates/reading/src/lib.rs              # только pub mod существующих слоёв
crates/reading/src/domain/mod.rs       # сущности, правила; без axum/sqlx
crates/reading/src/schemas/mod.rs      # входы/выходы и validated DTO
crates/reading/src/services/mod.rs     # use-case и порт
crates/reading/src/repositories/mod.rs # PostgreSQL SQLx + fake только для unit
crates/reading/src/api/mod.rs          # собственный Axum router()
crates/reading/tests/reading_pg.rs     # реальная PostgreSQL
```
Файлы/папки без реализации не создавать. Не писать SQLx за пределами `repositories`. Схема зависимостей: feature → contracts; `web` зависит от всех нужных feature, его код только связывает `Arc<dyn Trait>` и router. В `contracts` определить для каждого публичного порта actor/издатель/entitlement/reader *контракт*, не реализацию. Не использовать HTTP localhost вместо внутренних вызовов.

**Слои:** `api` — формы/JSON, Origin/CSRF, статус/заголовки; `services` — права, входная нормализация и бизнес-операция; `repositories` — SQL и атомарное сохранение по команде `service`; `schemas` — типы/пределы; `domain` — инварианты без HTTP/SQL. Сложную транзакцию выполнить **одной атомарной функцией репозитория**, вызываемой сервисом. Не пытаться держать `sqlx::Transaction` в сервисном слое, который не импортирует SQLx.

## 3. Типы и действующие инварианты

| Предмет | Выбранное представление | Граница и ошибка |
| --- | --- | --- |
| UserId, BookId, ChapterId, ChapterVersionId, PurchaseId | положительный `i64` внутри Rust/PG (`BIGINT`). Новые JSON команды представляют ID **десятичной строкой**; существующий `GET /api/v1/books` временно сохраняет свой числовой `id`/массив для обратной совместимости. | `0`, отрицательные ID, числа JSON вне безопасного диапазона — 422 для новых команд. Старую форму не менять молча. |
| Время | PG `TIMESTAMPTZ`, на проводе UTC RFC3339 | нет локального времени внутри транзакций, расписание хранит UTC instant |
| Деньги | `i64` минимальных единиц RUB (копейки), без `f32/f64` | баланс ≥0, сумма >0, цена сверяется с ожидаемой редакцией |
| Key повторной команды | Base64url без padding от 32 случайных байтов = **ровно 43 символа**, как в исходных SQLx миграциях | одинаковый Actor+key+payload ⇒ вернуть сохранённый результат; тот же key+другой payload ⇒ 409 |
| Книга | уникальность (`source_identity`, нормализованный `target_language`) | slug не идентичность, разные переводы разных языков отдельны |
| Обычная глава | Markdown не более **50 000 символов** (PG `char_length`), author-форма не более **1 МиБ HTTP body** | 422 за превышение; XSS/сырая HTML-разметка очищаются |
| Редакция | `ChapterVersionId` — immutable; публикация указывает текущую редакцию, покупка закрепляет свою | UPDATE body существующей версии запрещён даже для автора |
| Права | Actor от A, публикация B, entitlements D, чтение C | при неизвестном праве — 404/403 без выдачи content |
| Читательская позиция | `(version_id,block_index,char_offset)`; offset в UTF-16, `1..200` блоков, `0..100000` offset — совместимые исходные границы | старый auto-save не откатывает более свежую позицию; прогресс не entitlement |

**Примечание по лимитам:** разрешённый размер картинки/архива раньше **не был утверждён**. Для нового TOR вводится технический безопасный предел как часть этой спецификации (см. `docs/roadmap/data-and-events.md`), с отдельными тестами; он не объявляется историческим фактом NEwBOO.

## 4. HTTP: сохраняемые URL и новое JSON-пространство

**Существующее поведение bokarust, которое не ломаем при переносе без теста совместимости:**

| Метод | Маршрут | Форма / тело | Лимит |
| --- | --- | --- | --- |
| GET | `/`, `/catalog`, `/books/{id}`, `/reader/{id}` | HTML, публичная видимость | No-store для текста |
| GET | `/api/v1/books?limit=20` | JSON **массив** `[{id:number,title,target_language}]`, max `limit=100` | не заменять его внезапно объектом |
| GET/POST | `/register`, `/login` | `email`/`display_name`/`password`/`csrf_token` (вход без имени) | 4096 байт |
| GET | `/me`, `/me/books`, `/me/books/{book_id}`, `/me/library` | HTML authenticated | No-store |
| GET/POST | `/books/new`, `/books/{book_id}/chapters`, `/books/{book_id}/chapters/{chapter_id}` | `source_identity,target_language,title,command_key,csrf_token` / `number,expected_revision,title,body_markdown,access_level,price_amount,command_key,csrf_token` | 1 МиБ |
| GET/POST | `/read/{chapter_id}`, `/read/{chapter_id}/place`, `/read/{chapter_id}/place/auto` | `csrf_token,version_id,block_index,char_offset` | 2048 байт |
| GET | `/me/purchases`, `/me/purchases/{chapter_id}` | HTML authorized immutable purchase | No-store |
| GET/POST | `/chapters/{chapter_id}/buy` | `csrf_token,command_key,expected_version_id,expected_price_minor` | 2048 байт |
| GET | `/healthz` | `ok` = живой процесс, **не доказательство PostgreSQL** | — |

Сверка исходников: [routes](https://github.com/calex794/bokarust/blob/main/crates/web/src/routes.rs), [auth](https://github.com/calex794/bokarust/blob/main/crates/web/src/auth.rs), [author](https://github.com/calex794/bokarust/blob/main/crates/web/src/author.rs), [progress](https://github.com/calex794/bokarust/blob/main/crates/web/src/reading_private.rs), [checkout](https://github.com/calex794/bokarust/blob/main/crates/web/src/checkout_private.rs).

**Новые JSON-команды TOR — одно выбранное семейство, без второго набора семантики:** `/api/v1/auth/*` (только если нужен JSON-клиент, forms остаются), `/api/v1/books/{id}/chapters`, `/api/v1/reading/*`, `/api/v1/library/*`, `/api/v1/purchases/*`, `/api/v1/community/*`, `/api/v1/import-jobs/*`. Их подробные команды и поля см. [документ контрактов API](api-v1.md). Не делать одновременно два разных write-path: JSON и HTML вызывают **один use-case**.

### Статусы и ошибка JSON

```json
{"error":{"code":"VERSION_CONFLICT","message":"Данные были изменены","request_id":"01ABC..."}}
```

- 400: синтаксис/неразбираемый cursor; 401: требуется вход; 403: известный вошедший пользователь не имеет роли; 404: несуществующая **или скрытая** книга/глава; 409: optimistic lock или key reuse с иным payload; 413: байты превышают лимит; 422: неверное поле; 429: частота; 503: БД/внешний сервис недоступен. Не различать 403 vs 404 способом, раскрывающим факт приватной книги.
- `X-Request-Id` во всех ответах, для приватных `Cache-Control: no-store`. Не складывать body, пароль, session token, chapter text, персональные IP в журналы. У HTTP запросов с cookie на изменение — Origin + CSRF проверка; только на тесте с прямой машинной авторизацией допускается другой договор, не обход.
- `/healthz` показывает живой процесс, новый `/readyz` делает `SELECT 1` к PostgreSQL и проверяет применённую версию миграции. Код 200 только после успешной готовности. Наличие demo-хардкода не считается reader или readiness.

## 5. Критическая запись и роли (проверено в исходной Rust основе)

**A:** `users`, `sessions`, `auth_attempts`; регистрация+первая сессия атомарно. Никаких проверок прав по email; только Actor из серверной cookie. Ссылка: [001 accounts](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610120001_create_accounts.sql), [003 auth_attempts](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610120003_auth_attempts.sql).

**B:** `books`, `chapters`, `chapter_versions`, `chapter_publications`, `book_create_commands`, `chapter_save_commands`, `chapter_first_release_events`. Сохранение через revision CAS; новый hash версии может переиспользоваться, но опубликованную старую версию нельзя изменять. Публикация, версия и durable event — одна транзакция. Ссылка: [004 reading](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610120004_public_reading.sql), [005 author](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610120005_author_publication.sql).

**C:** `reader_book_positions` и новая `reader_purchased_positions` для BKR-134 (см. [схему и миграции](data-and-events.md)). Права остаются в D; C не пишет entitlements. Ссылка: [006 progress](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610120006_reader_progress.sql), [reader offsets](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610130001_reader_offsets.sql).

**D:** `chapter_purchase_facts`, `library_purchased_chapters`, `reader_wallet_accounts`, `reader_wallet_entries`, `chapter_purchase_commands`, `chapter_purchase_revocations`. Покупка с блокировкой кошелька, проверкой цены/редакции и идемпотентности в **одной** транзакции. Денежная история append-only. Ссылка: [002 purchased](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610130002_purchased_chapters.sql), [003 checkout](https://github.com/calex794/bokarust/blob/main/crates/adapters/migrations/202610130003_wallet_checkout.sql).

**E/F/G:** владеют своими новыми таблицами; матрица owners, ключи и события зафиксированы отдельно в [data-and-events.md](data-and-events.md). Ни один из них не может выдать право покупки или создать версию публикации напрямую. Создатель события B/D пишет **свой** бизнес-факт и общий outbox при commit; G управляет **доставкой/lease**, но не меняет бизнес-факт.

## 6. Что запрещено выдавать за готовое

- **S053–S056**: сцены/маркетплейс — заморожено до отдельного продуктового решения. Не создавать редактор сцен. **S040/S067/S069** — исторические не переносимые задачи. Всего 62 первично назначенных сценария, включая требующие уточнения.
- **S041 Boosty:** в NEwBOO имеется диагностический probe, нет подтверждённого завершённого платёжного провайдера. Финансовый реальный endpoint оставить **отключённым**: нет зачисления до подписанного/проверенного callback+ledger. Имитация подтверждения допустима только в тесте изоляции.
- **S003 Яндекс OAuth:** парольная авторизация не доказывает OAuth. Callback готовить под state/nonce, но внешнее включение — только после действительных credentials/конфигурации и теста настоящего round-trip. Нет зависимости от включённого Яндекса для сайта.
- **S042/S043 реклама:** в истории NEwBOO рекламные поставщики хранилища неполны и rollout был отключён. Можно реализовать модели/тестовые черновики; боевую выдачу и списания не включать до реального impression+anti-fraud+settlement.
- **S068/S069:** материалы из несоединённых PR/документов — не считать уже действующим функционалом исходного `main`.
- Нельзя считать D1-визуализацию текущего `tor/main` настоящей базой: на 25.10 там S0 demo с жёстко заданными книгами в памяти. Обязательная первая работа I — заменить demo режим на настоящий PostgreSQL путь; до этого деньги/вход отключены правильно.

## 6A. Обязательное решение: прямой S3 для платных глав с первого релиза

Принята целевая архитектура [paid-delivery-v1](../contracts/paid-delivery-v1.md) по [разбивке B/C/D/G/H/I в Tela](https://telawiki.com/spaces/430/pages/7736/tor-s3). Платная глава хранится в приватном S3 как immutable AES-256-GCM ciphertext с отдельным wrapped DEK для каждой версии. PostgreSQL/D подтверждает право и pinned edition, C выдаёт signed GET и scoped DEK (TTL 90 секунд), H скачивает ciphertext **напрямую S3→browser**, расшифровывает через WebCrypto; API Rust не пересылает платный Markdown читателю. Бесплатные опубликованные главы могут идти через public S3/CDN. Платные иллюстрации также приватны. Presigned URL допускает повторное использование до TTL и копирование клиентом — это не абсолютный DRM. Обязательные негативные тесты: отсутствие права/отзыв, чужой объект, immutable V1 после V2, CDN free→paid, PUA-совместимость, отказ и восстановление S3/PG. Текущий TOR ещё этого не реализует. В `tasks.tsv` остаётся 117 строк, детальные S3 шаги входят в существующие B/C/D/G/H/I.

## 7. Передача между исполнителями

На каждый пакет задач: **ID задачи + S/F → исходный URL/SHA → ожидаемые точные URL/SQL/contract → файлы по владельцу → тесты успеха/отказа/гонки → PR на обновлённом main → успешный SHA/CI → запись в старую матрицу Notion**. Точка I решает только конфликты инфраструктуры/контрактов и не заменяет предметного владельца.

Порядок проверки и запуска детализирован в [tests-and-handoffs.md](tests-and-handoffs.md); подробная очередь каждой линии — [директория lane runbooks](lanes/). Не откладывать архитектурное проектирование исполнителю. Если внешняя система недоступна, отдавать безопасную отключённую конфигурацию с documented blocker, а не импровизировать API провайдера.


## 8. Решения PREP-01…08 на 26.10.2026

Новая единственная точка соглашений: [index](README.md), [FIN-01](../contracts/finance-ad-exact-v1.md), [FIN-02](../contracts/refunds-payouts-v1.md), [ACL](access-authorization-contract.md), [REC golden](../contracts/recommendations-golden-v1.md), [private S3 / file upload](../contracts/import-storage-v1.md), [events/RIL](../contracts/events-analytics-v1.md), [OpenAPI](../contracts/openapi-v1.json), [release flags](../contracts/release-flags-v1.json). Все технические default’ы нормативны для исполнителей, **но это не доказательство реализованной подсистемы**. Исторический F021/S024 crawler заморожен; предметная часть G — файл/обложка/S3/jobs. Бизнес-прайс, доля publisher, refund deadline и cash-out provider не выдумываются: брать immutable конфигурацию предложения либо feature disabled/fail closed.
