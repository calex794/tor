# TOR — PostgreSQL, события, файлы и лимиты: точное задание

**Рабочий норматив для реализации, не отчёт о готовой БД.** База TOR на 25.10.2026 ещё не создана; нельзя выдавать DDL ниже за уже применённые миграции. Источник проверенных таблиц — девять SQL-файлов из `bokarust/crates/adapters/migrations/`. Перенести их в TOR *в сохранённом порядке* через **линейную** систему миграций и далее только расширять новыми файлами, не изменять уже применённый SQL.

## 1. Порядок и авторитет миграций

| Порядок | Брать за исходник | Таблицы / инварианты | Проверка |
| --- | --- | --- | --- |
| 001 | `202610120001_create_accounts.sql` | `users` (нормализованный email и Argon2id), `sessions` (SHA256 token, TTL, revocation) | регистрация в PG + сессия одним commit |
| 002 | `202610120002_create_books.sql` | `books`, уникальность (`source_identity`, `target_language`), индекс public | две одинаковые книги в одном языке → конфликт |
| 003 | `202610120003_auth_attempts.sql` | `auth_attempts`, общий rate-limit без сырых IP/email | 2 процесса не обходят лимит |
| 004 | `202610120004_public_reading.sql` | `chapters`, immutable `chapter_versions`, `chapter_publications`, admin_hold/deletion | прямой URL draft 404 без текста |
| 005 | `202610120005_author_publication.sql` | owner, optimistic edit, prices, command tables, release fact | один источник author, повтор команды идемпотентен |
| 006 | `202610120006_reader_progress.sql` | `reader_book_positions` со ссылкой на точную edition | после restart прочитанное место возвращается |
| 007 | `202610130001_reader_offsets.sql` | `char_offset` — UTF-16 offset | emoji/кириллица сохраняют правильную позицию |
| 008 | `202610130002_purchased_chapters.sql` | покупка и закреплённая edition, revocation, immutable facts | право нельзя подделать через progress |
| 009 | `202610130003_wallet_checkout.sql` | RUB wallet+ledger+idempotency purchase command | два concurrent buy → одно списание |
| 010 (новая) | `reader_purchased_positions` | позиция для купленной версии, `BKR-134` | published V2 не теряет V1/P |
| 011 (новая) | `outbox_events` | один durable outbox, lease/fence, dedupe | событие не потеряно после commit |
| 012+ | дополнительные миграции **от соответствующих владельцев** E/F/G, команд B | отдельные новые таблицы, индексы, FK/связи; порядок через I | воспроизводимая чистая установка и миграция старой схемы |

Точные файлы-доноры: [SQL-migrations bokarust](https://github.com/calex794/bokarust/tree/main/crates/adapters/migrations). Новый путь: `tor/migrations/YYYYMMDDNNNN_<action>.sql`. **Не использовать YDB DDL.** SQLx version каждого файла уникален. В CI проверять обе последовательности: пустая PG → все миграции; PG после 009 → последующие миграции с реальными данными. Не удалять historical rows ради исправления constraint.

## 2. Неподлежащие импровизации новые таблицы

### C: купленная позиция BKR-134

Добавить в миграции 010:

```sql
CREATE TABLE reader_purchased_positions (
    user_id BIGINT NOT NULL,
    purchase_id BIGINT NOT NULL,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    block_index INTEGER NOT NULL CHECK (block_index BETWEEN 1 AND 200),
    char_offset INTEGER NOT NULL DEFAULT 0 CHECK (char_offset BETWEEN 0 AND 100000),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, purchase_id),
    FOREIGN KEY (purchase_id, user_id, chapter_id, version_id)
        REFERENCES chapter_purchase_facts(id, user_id, chapter_id, version_id)
        ON DELETE RESTRICT
);
CREATE INDEX reader_purchased_positions_recent
    ON reader_purchased_positions (user_id, updated_at DESC, purchase_id DESC);
```

Обновление: `UPDATE ... SET block_index=?, char_offset=?, revision=revision+1, updated_at=now() WHERE user_id=? AND purchase_id=? AND revision=?`; `0 rows` = проверить revoked/access или `409 CONFLICT` с текущей revision. Создание при отсутствии — `INSERT ... ON CONFLICT ... DO NOTHING` и затем CAS. На каждом GET/POST обязательно проверить **текущее** отсутствие `chapter_purchase_revocations`, принадлежность `purchase_id` и совпадение version_id. `progress` сам по себе никогда не участвует в `CanRead`.

**Старый `reader_book_positions` для бесплатного пути не переписывать автоматически.** Совместимость с существующим `/read/{id}/place` сохранять; платную позицию принимать в отдельном обработчике, который знает `purchase_id` и `revision`. У потока C+D должен быть тест: V1 куплена → V2 опубликована → позиция V1/P восстанавливается.

### I/G: общий outbox для последствий

Добавить в миграции 011:

```sql
CREATE TABLE outbox_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type TEXT NOT NULL,
    schema_version SMALLINT NOT NULL CHECK (schema_version > 0),
    aggregate_kind TEXT NOT NULL,
    aggregate_id BIGINT NOT NULL,
    dedupe_key TEXT NOT NULL UNIQUE,
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    required_consumers TEXT[] NOT NULL CHECK (cardinality(required_consumers) > 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_until TIMESTAMPTZ,
    lease_epoch BIGINT NOT NULL DEFAULT 0,
    delivered_at TIMESTAMPTZ,
    dead_at TIMESTAMPTZ
);
CREATE INDEX outbox_due_idx ON outbox_events (next_attempt_at, created_at, id)
    WHERE delivered_at IS NULL AND dead_at IS NULL;
CREATE TABLE event_deliveries (
    consumer TEXT NOT NULL,
    event_id UUID NOT NULL REFERENCES outbox_events(id) ON DELETE RESTRICT,
    delivered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer, event_id)
);
```

**Владение:** I — структура и версия outbox; B/D/E и прочие владельцы вправе добавить событие **в свою бизнес-транзакцию** через согласованный insert по полям выше (явное техническое исключение к «пишем только свои таблицы»); G — claim/retry/последующая доставка, а не право задним числом изменять состояние покупки или главы. **Для каждого опубликованного event владельцу обязателен список `required_consumers`, снимок имён обработчиков на момент записи.** Например `ChapterPublished` → `["search-index-v1","subscription-inbox-v1"]`; `PurchaseCommitted` → `["earnings-projection-v1"]`; `ImportCompleted` → `["import-status-v1"]`. Имена/version меняются только согласованной миграцией контракта, не в runtime-памяти случайного процесса.

**Публикации:** `chapter_first_release_events` из миграции 005 — ровно один **исторический бизнес-факт** первого выпуска главы, а **outbox `ChapterPublished`** создаётся для *каждой новой опубликованной редакции* с `dedupe_key="chapter-published:<chapter_id>:<version_id>"`, включая первый выпуск. Повтор команды той же версии не публикует второй event; V1→V2 публикует новый event и обновляет поиск/подписки. Для покупки `dedupe_key="purchase:<purchase_id>"`. Любой consumer использует уникальную пару `(consumer,event_id)` для идемпотентности.

**Fan-out и глобальный `delivered_at`:** G сначала атомарно claims одно event с `lease_epoch`, затем вызывает **каждого** из `required_consumers`, который ещё отсутствует в `event_deliveries`. Успех каждого потребителя записывается отдельной `INSERT ... ON CONFLICT DO NOTHING` в `event_deliveries` (в одной PG-транзакции с его локальным эффектом, когда эффект хранится в той же PG). **`outbox_events.delivered_at` устанавливать только если для всех имён из `required_consumers` имеется row `(consumer,event_id)`.** Если один потребитель упал, доставленные другим не повторять; event остаётся ожидающим. Результат внешней сети без подтверждённого idempotency-key нельзя считать delivered. Сбой между внешним действием и записью delivery может привести к повтору, поэтому downstream обязан дедуплицировать. Для lease окончательная установка delivered_at проверяет `WHERE lease_epoch=?`.

Worker claim: выбор `WHERE delivered_at IS NULL AND dead_at IS NULL AND next_attempt_at<=now() AND (lease_until IS NULL OR lease_until<now()) FOR UPDATE SKIP LOCKED LIMIT N`, затем атомарное увеличение `lease_epoch`, `attempts` и установка `lease_until`. При завершении `UPDATE ... WHERE id=? AND lease_epoch=?`: старый владелец не может удалить запись нового. Повтор после таймаута разрешён; нужно дедуплицировать эффект у consumer. Backoff: минимум 5s, максимум 1h, с jitter; после 12 попыток → `dead_at` + диагностика. Retry запрещён для неидемпотентного внешнего API без ключа.

**Имена payload v1:**
- `ChapterPublished`: `{chapter_id,book_id,version_id,published_at,first_release}`. Outbox insert **вместе с публикацией до commit**, а обработка только после commit; каждый V1/V2 имеет собственный ключ события, первый выпуск обозначен `first_release=true`.
- `PurchaseCommitted`: `{purchase_id,actor_id,chapter_id,version_id,amount_minor,currency:"RUB"}`. Не пересылать body главы, пароль или платёжные данные.
- `SubscriptionChanged`: `{actor_id,target_id,active}`.
- `ImportCompleted`: `{job_id,book_id,created_chapter_count}`.
При изменении формата — новый `schema_version`, поддержка старых сообщений до опустошения очереди.

## 3. Таблицы и владельцы без двусмысленности

| Область/пакет | Авторитетные таблицы/сущности | Чего не имеет права писать |
| --- | --- | --- |
| A `accounts` | users, sessions, auth_attempts, account_settings, external_identities, security_events | purchases/reading progress |
| B `books` + `teams` | books, chapters, chapter_versions, chapter_publications, book_create_commands, chapter_save_commands, first release, team memberships, owner/share revisions | wallet/entitlement/search index |
| C `reading` | reader_book_positions, reader_purchased_positions, library_items, bookmarks, reading_history | chapter_versions, purchases, ledger |
| D `commerce` | wallet accounts/entries, purchase commands/facts/revocations, payouts, refunds, layers/entitlements, settlement snapshots | content body, reader positions |
| E `discussions`/`social`/`clubs`/`support` | comments/anchors, threads, reactions, subscriptions, relationships, collections, inbox, clubs, moderation, support tickets | purchased rights, authoritative chapter |
| F `search`/`recommendations`/`referrals` (+ `ads`/`analytics` в реестре I) | public search/read projections, ratings/recommendations, SEO generations, ad campaigns, referral attribution, aggregate metrics | wallet entries, books lifecycle |
| G `imports`/`ingest` (+ `media`/`jobs` при реальном росте через I) | source URLs, import jobs/checkpoints, file references, delivery attempts/worker leases; очередь обрабатывает общий outbox | books versions, wallet postings |
| H `web` | только шаблоны, маршрутизация и состояние UI | авторитетные бизнес-таблицы |
| I `contracts`/`web`+infra | schema migrations, `outbox_events` инфраструктура, CI/smoke/readiness, wiring | бизнес-правила, которые принадлежат A–G |

**Внешние ключи на чужую таблицу допустимы** — это защита целостности, но не разрешение писать чужие записи. На чтении чужого состояния предпочитать опубликованный typed-port. В критичной транзакции D может читать `chapter_versions`/price через `SELECT ... FOR SHARE`, но любые денежные записи только D и его SQL adapter.

### Обязательная схема объектов S3 для первой версии TOR (проект миграций I)

[Единый норматив `paid-delivery-v1`](../contracts/paid-delivery-v1.md): G владеет таблицей `storage_objects` (UUID PK, unique object_key, private/public bucket, state pending/ready/attached/deleting, encrypted_dek, wrap_nonce, kek_id, nonce, aad, ciphertext_hash, size, lifecycle). B владеет ссылкой `chapter_versions.storage_object_id` и приватными `chapter_version_assets`; D сохраняет существующую атомарную модель `chapter_purchase_facts`/`library_purchased_chapters`/revocations; C — `reader_purchased_positions`. I создаёт миграции в линейном порядке. Ключи DEK в PG только в wrapped-виде, KEK вне репозитория и БД. `pending → private S3 PUT → HEAD/checksum → PG publish+attach+outbox`; невозможность атомарной транзакции PG+S3 закрывается orphan GC с fencing. Публичный Markdown только для опубликованной бесплатной редакции; переход free→paid требует delete public origin + CDN purge, но копии ранее опубликованного текста уже нельзя отозвать. GC не удаляет купленную закреплённую V1 после появления V2. Это **предстоящие изменения схемы**, не утверждение о применённых миграциях.

## 4. Лимиты, включая ранее неутверждённые

Это **выбранные ограничения новой реализации TOR**, не фактические исторические лимиты NEwBOO:

| Поверхность | Ограничение | Реализация/проверка |
| --- | --- | --- |
| Auth form | ≤4096 **байт** запроса | `DefaultBodyLimit`; 413 |
| Author form | ≤1 048 576 байт тела, отдельная глава ≤50 000 **Unicode chars** | граничные тесты `limit-1/limit/limit+1` |
| Position / checkout | ≤2048 байт каждый | CSRF, 413 и owner check |
| Обложка и другое изображение | **≤8 МиБ = 8 388 608 байт** оригинала, декодированная площадь ≤24 млн пикселей | ограничить до decode, MIME и magic bytes, metadata stripping |
| Архив импорта | **≤32 МиБ = 33 554 432 байта** сжатого потока, сумма распаковки ≤256 МиБ, ≤5000 файлов, коэффициент разжатия ≤50 | потоковый разбор, лимит памяти не выше 64 МиБ для одной задачи, 413/422 |
| Внешние URL | `https`, домены из allowlist источников; DNS IP/range проверяется **после** resolve и при каждом redirect | запрет loopback/private/link-local/metadata адресов; max 5 redirect; timeout connect 5s, total 30s |
| JSON обычной команды | ≤64 КиБ, если домен не указал иной явный лимит | возвращать 413, не указывать заголовки content-type «бесконечно» |
| Каталог | существующий `GET /api/v1/books limit`: 1–100, default 20; новый cursor-путь: limit 1–100, max token 512 байт | keyset, индексы, фильтр видимости до limit |
| Обработка отправки уведомлений | пачка 100 получателей, cursor до конца без потери 101-го/201-го | тест на 201, повтор фрагмента не создаёт 2 Inbox entries |
| Данные в HTML/логах | нет паролей, session cookie, главы платной редакции и приватной цитаты | fuzz/XSS/ACL тест, CSP/Cache-Control |

Снижение максимальных размеров не должно молча удалять старое содержимое; импорт с превышением отклоняется **до записи** книги. Для поднятия лимита требуется отдельная проверка ресурса на 1 vCPU/1 GB VPS, явное изменение контракта и тест.

## 5. PostgreSQL-покупка, один commit, без догадок

В обработчике `checkout` бизнес-операция принимает `actor_id,chapter_id,expected_version_id,expected_price_minor,command_key`.

1. Начать транзакцию `BEGIN`.
2. `INSERT chapter_purchase_commands(actor,command_key,expected_version,expected_price)`; при конфликте ключа запросить прежнюю payload и receipt, тот же payload вернуть без списания, другой → 409.
3. Проверить издательскую/читательскую видимость, совпадение `chapter_publications.version_id` с `expected_version_id`, цену и доступность; зафиксировать согласованное правило чтения, например `FOR SHARE` на цене/главе.
4. Заблокировать строку кошелька `SELECT ... FOR UPDATE`, проверить достаточность.
5. Проверить, что у пользователя нет уже active purchase той же главы/редакции; повтор другой командой должен дать понятный отказ без списания.
6. `UPDATE reader_wallet_accounts` с новым неотрицательным остатком.
7. `INSERT chapter_purchase_facts`; существующий DB trigger установит pinned `library_purchased_chapters`.
8. `INSERT reader_wallet_entries` с отрицательным delta и уникальным reference; записать результат команды и `PurchaseCommitted` в `outbox_events`.
9. `COMMIT`; при неопределённом ответе после commit клиент использует **тот же ключ** и получает исходный receipt (не отправляет случайный новый ключ).

**Тесты:** 2 одновременных buy одного экземпляра, 2 одинаковых ключа, key+другой payload, insufficient funds, unknown commit, published V2 after purchasing V1, revoked purchase, zero/negative amount, mismatch currency. Все на настоящей PG, не только mock.

## 6. Открытые внешние зависимости — уже выбранное безопасное поведение

- Provider Boosty / S041: `enabled=false` до проверенного webhook/подписей, бухгалтерской транзакции и возмещения; endpoint **не** создаёт `verified_funding` без подписи и идемпотентности.
- Яндекс OAuth / S003: `enabled=false` без действительных ключей и реального callback теста; парольная авторизация доступна независимо.
- Ads / S042–S043: `delivery_enabled=false`, `settlement_enabled=false` до независимых доказанных PostgreSQL-providers и антифрода. Можно создать/редактировать кампанию в sandbox, но не включать списания.
- Сцены / S053–S056: `enabled=false`, не строить API/таблицы редактора/маркетплейса без отмены продуктовой заморозки.
- Демо на VPS только RAM не принимает пользовательские записи: `TOR_DEMO_MODE` отделён от полноценной сборки с PG; бета денег/логина проходит только при постоянной базе.

Эти решения позволяют исполнителю работать без вопроса «какой внешний сервис брать?». Внешняя активация отдельно от переноса ядра, а «100% без исключений» пока не утверждено.
