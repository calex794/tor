# TOR — HTTP API v1: выбранные маршруты, поля, статусы и контракты

**Назначение:** исполнителям A–H не придумывать имена маршрутов, сериализацию, ограничения и семантику ошибок. Этот документ — **контракт для новой реализации TOR**, а не утверждение, что маршруты уже доступны. Сохраняемые из `bokarust` HTML URL указаны в [decisions-v1](decisions-v1.md); они вызывают те же службы, что JSON API. Поле `id` в новых JSON API — положительная десятичная строка, время — UTC RFC3339; деньги — целое число копеек.

## Общие договорённости

- Каждый JSON endpoint возвращает `Content-Type: application/json`; ошибки в конверте `{"error":{"code":"...","message":"...","request_id":"..."}}`.
- Внешний пользователь получает только документированный публичный response DTO, а не SQL row. **Единственный ключ повтора JSON API** — заголовок `Idempotency-Key` (строго 43 символа base64url без padding из 32 случайных байтов) у всех отмеченных ниже создающих/изменяющих операций; **в JSON body ключ не дублируется и не принимается** (лишнее поле = 422 `UNKNOWN_FIELD`). Тот же actor+key+canonical payload → исходный ответ и тот же ID; тот же ключ+другой payload → 409 `IDEMPOTENCY_CONFLICT`. Для HTML форм донорский `command_key` остаётся скрытым полем и преобразуется в **тот же use-case**, отдельного HTTP-заголовка от обычной формы не требуют.
- Поля строк, length, диапазоны проверять **до** бизнес-команды и повторно ограничениями БД. Статусы: 400 malformed JSON/cursor, 401 нет сессии, 403 роль отказана для известного объекта, 404 скрытый/неразрешённый объект, 409 конфликт версии/повтора, 413 слишком большое тело, 422 семантически неверные поля, 429 ограничение частоты, 503 инфраструктурный отказ. Никакая 404-страница/JSON не содержит скрытой цитаты или metadata.
- HTML и JSON команды разделяют один `service`. У cookie mutating POST/PATCH/DELETE всегда Origin и CSRF. У JSON API не делать обход «поскольку API». `request_id` генерируется на границе `web`, redacted logs.

**Правило валидатора:** для JSON POST/PATCH, помеченного идемпотентным, `Idempotency-Key` отсутствует → 422 `IDEMPOTENCY_KEY_REQUIRED`; неправильная длина/алфавит → 422 `INVALID_IDEMPOTENCY_KEY`; повтор того же actor+key с другим телом → 409. Внутреннее имя `command_key` в PG совместимо с донорскими таблицами и заполняется значением из заголовка, не из JSON тела. Для фонового job применяется тот же ключ в объекте команды. Нельзя возвращать результат по ключу одного Actor другому. JSON не должен незаметно игнорировать неизвестные поля.

## A — аккаунты (владелец tor-accounts)

| Метод+путь | Тело/выход | Право; успех | Отказ и тест |
| --- | --- | --- | --- |
| POST `/register`, POST `/login`, POST `/logout` | Уже существующие `application/x-www-form-urlencoded`: `email,display_name,password,csrf_token`; login без display; logout только csrf | Гость/владелец; серверная cookie, redirect/HTML. **Сохранить из bokarust** | duplicate email 409/форменный отказ; неверный Origin и cookie; лимит 4096 байт |
| GET `/me` | HTML личный кабинет | Текущая сессия, no-store | Гость не видит данные |
| GET `/api/v1/me` | `{id,display_name,handle,adult_enabled,delivery_settings}` | Только текущий Actor, 200 | 401 без cookie, никакого password_hash |
| PATCH `/api/v1/me` | `{display_name?,handle?,adult_enabled?,delivery_settings?,expected_revision}` | Владелец, 200 + новая revision | 409 stale revision; невозможно выставить чужой UserId |
| POST `/api/v1/me/password` | `{current_password,new_password,csrf_token}` | Владелец; revoke previous sessions | 401/403, 422 weak password, старый token недействителен |
| GET/POST `/auth/yandex/*` | `state,nonce,code` по OAuth; вход disabled до config | callback связывает только verified provider subject | неверный state/callback даёт отказ, matching email не proof |
| GET `/api/v1/admin/auth-events` | bounded chronological page | Только admin | чужой 403, PII/tokens redacted |

Никакой client-provided `actor_id` не принимается как право пользователя. В основе — `users.email_normalized`, `sessions.token_hash` и `auth_attempts.key_hash` из донорских SQL.

## B — книги, главы, команды (tor-books, tor-teams)

| Метод+путь | Request / response | Права/commit | Негативный случай |
| --- | --- | --- | --- |
| POST `/api/v1/books` | `{source_identity,target_language,title,visibility:"draft"}` → `{id,revision:1}` | Actor с правом авторства; одна каноническая книга | same identity/language 409; чужой не редактирует |
| PATCH `/api/v1/books/{book_id}` | `{title?,description?,genre_ids?,visibility?,expected_revision}` → `{id,revision}` | Владелец/автор с разрешением | stale rev 409; blocked/draft не раскрывать гостю |
| POST `/api/v1/books/{book_id}/chapters` | `{chapter_no,title,body_markdown,access_level:"free|paid",price_minor,expected_book_revision}` → `{chapter_id,version_id,edit_revision}` | Пользователь с правом редактирования, не использовать JSON-суммы в float | body>50000 chars/price mismatch 422; повтор same key даёт тот же ID |
| PATCH `/api/v1/chapters/{id}` | `{title,body_markdown,expected_revision}` → `{version_id,edit_revision}` | Запись **новой** immutable версии | 409 concurrent editor; тело V1 никогда не меняется |
| POST `/api/v1/chapters/{id}/publish` | `{expected_revision,version_id}` → `{published_version_id,published_at}` | Владелец + атомарный outbox event | 2 одновременных publish ≠ 2 release events |
| POST `/api/v1/chapters/{id}/schedule` | `{version_id,publish_at_utc,expected_revision}` → `{schedule_id,status:"scheduled"}` | Владелец, worker через G | отменённое расписание, отозванная роль, просроченный lease — не публиковать |
| DELETE `/api/v1/chapters/{id}/schedule` | `{expected_revision}` → `204` | Только actor с правом управления | stale scheduler не выпускает |
| GET/POST `/api/v1/teams`, POST `/api/v1/teams/{id}/invites` | `{name}`, `{invitee_id,role}` → id/статус | TeamOwner, инвайт подтверждает приглашённый | чужой не принимает, двойной invite безопасен |
| POST `/api/v1/books/{id}/transfer` | `{new_owner_id,expected_revision}` → новый owner | Транзакция membership+owner | 2 параллельных transfer не дают двойное владение |
| GET `/api/v1/teams/{id}/analytics.csv` | потоковый CSV | Только разрешённый член команды | чужой 403, большой экспорт bounded |

**Сохраняемые HTML:** `/books/new`, `/me/books`, `/books/{book_id}/chapters` и формы редактирования, существующие в bokarust. Не внедрять JSON-путь как второй автономный writer: и формы, и JSON вызывают одну доменную операцию.

## C — чтение, прогресс и личная библиотека (tor-reading)

| Метод+путь | Request/response | Право/инвариант | Тест |
| --- | --- | --- | --- |
| GET `/books/{id}`, `/reader/{chapter_id}` | SSR published card и только бесплатная текущая версия | 200 только public; no-store для текста | draft и admin hold 404, в ответе нет markdown |
| GET `/api/v1/books/{id}/toc?limit=100&after=token` | `{items:[{chapter_id,chapter_no,title,access_level}],next_cursor}` | только разрешённая публичная мета, без body | 501 глава: все доступны постранично, gap/duplicate нет |
| GET `/api/v1/chapters/{id}/read?purchase_id=...` | `{chapter_id,version_id,body_markdown,access:"free|purchased"}` | бесплатная published ИЛИ D-грант на конкретную V1; если purchase_id, проверить owner+revoked | 403/404 без body, cached forbidden |
| PUT `/api/v1/me/progress/free` | `{chapter_id,version_id,block_index,char_offset,expected_revision}` → `{revision,updated_at}` | actor читает разрешённую версию; не rollback newer progress | concurrent stale 409; отсутствие покупки не даёт платный доступ |
| PUT `/api/v1/me/progress/purchased` | `{purchase_id,chapter_id,version_id,block_index,char_offset,expected_revision}` → `{revision,updated_at}` | FK на `chapter_purchase_facts`, отзыв проверяется каждый раз | куплена V1, опубликована V2, возвращаем место V1/P |
| GET `/api/v1/me/library` | `{items:[...],next_cursor}` | только свой actor; purchased и bookmarks различать | нельзя увидеть чужую библиотеку |
| POST/DELETE `/api/v1/me/library/{book_id}` | 204/состояние | только личная закладка, не entitlement | добавление бесплатно не открывает paid |

Сохраняемые HTML: `/read/{chapter_id}`, `/read/{chapter_id}/place`, `/read/{chapter_id}/place/auto`, `/me/library`, `/me/purchases`. Для 500+ глав устранить source `LIMIT 200` из полного TOC, заменить ограниченным keyset API. Точность `char_offset` — UTF-16 (emoji test).

## D — покупка и деньги (tor-commerce)

| Метод+путь | Тело и ответ | Транзакция/право | Отрицательный тест |
| --- | --- | --- | --- |
| GET `/chapters/{chapter_id}/buy` | HTML с версией и ценой | текущая опубликованная версия/сессия | скрытая/отозванная глава недоступна |
| POST `/chapters/{chapter_id}/buy` | HTML form: `csrf_token,command_key,expected_version_id,expected_price_minor` | наследуемая атомарная purchase+ledger+entitlement | double submit 1 debit, limit 2048 байт |
| GET `/api/v1/chapters/{id}/offer` | `{chapter_id,version_id,price_minor,currency:"RUB",can_purchase}` | опубликованная версия, нет paid body | mismatch version/price 409 |
| POST `/api/v1/purchases` | `{chapter_id,expected_version_id,expected_price_minor}` → `{purchase_id,version_id,balance_minor}` | **один commit** wallet debit+ledger+fact+pointer/outbox | insufficient funds 409 без побочных записей |
| POST `/api/v1/purchases/batch` | `{items:[{chapter_id,version_id,price_minor}]}` | все линии одним PG commit, уникальные главы | одна ошибка откатывает всю покупку |
| POST `/api/v1/purchases/{id}/refund-request` | `{reason}` → `{request_id,state:"pending"}` | только владелец, не зачисляет сразу | чужой запрос 404, повтор один тикет |
| POST `/api/v1/admin/refunds/{id}/decide` | `{decision:"approve|reject",expected_revision}` | только финансовая admin роль, reversal ledger если approved | 2 concurrent admin не возвращают дважды |
| GET `/api/v1/me/wallet` | `{currency:"RUB",balance_minor,recent_entries}` | только владелец, no-store | не отдавать чужие записи |
| POST `/api/v1/me/payout-requests` | `{amount_minor,expected_revision}` | verified available earnings, не вывод через неподтверждённого внешнего провайдера | race и duplicate не увеличивают сумму |
| POST `/api/v1/wallet/topups/boosty` | **disabled**, ошибка `503 PROVIDER_NOT_CONFIGURED` до подписанного callback | никаких `verified_funding` без проверки внешнего подтверждения | неверная подпись/повтор нельзя зачесть |

**Нельзя** создавать topup через скрытый admin-mode под видом настоящей оплаты. На тестовом окружении подготовка баланса — SQL fixture, а не публичный endpoint. Внешние refund/payout требуют реальный договор и могут быть помечены blocked.

## E — социальные функции (tor-discussions,tor-social,tor-clubs,tor-support)

| Метод+путь | Данные | Право/инвариант |
| --- | --- | --- |
| POST `/api/v1/chapters/{id}/comments` | `{version_id,anchor?,body,spoiler,client_key}` → comment_id | Проверить A и C-разрешение на target; comment принадлежит единственной версии |
| GET `/api/v1/chapters/{id}/comments?limit&after` | bounded tree/excerpt, раскрытие spoiler по explicit действию | Нельзя утечь hidden paid anchor в guest payload |
| POST `/api/v1/comments/{id}/replies` | `{body,client_key}` → reply id | Один canonical thread, Inbox указывает исходный comment |
| POST `/api/v1/comments/{id}/reactions` | `{kind}` | Дубли не удваивают счётчики |
| POST/DELETE `/api/v1/users/{id}/follow` | 204 | Follow не создаёт право на платную главу |
| POST `/api/v1/users/{id}/block` | 204 | Новые DM/уведомления/личные данные закрываются сразу |
| GET/POST `/api/v1/me/collections` | private/public, book_ids, expected_revision | Только владелец правит, чужая приватная подборка 404 |
| GET/POST `/api/v1/me/messages` | peer_id, text, cursor | Только два участника, hidden previews отфильтрованы |
| GET/POST `/api/v1/clubs` | name, membership/role/status | Раздельные owner/member/moderator club и global admin |
| POST `/api/v1/reports`, GET/POST `/api/v1/admin/reports` | target_type/id, reason, decision | Только глобальный moderator/admin на решение |
| GET/POST `/api/v1/support/tickets` | guest claim token либо actor, message | Claim не позволяет захватить чужую переписку |
| GET `/api/v1/me/inbox` | bounded уведомления/упоминания с read markers | spoiler и текст платной главы не копируются в метаданные |

Формы H вызывают именно эти операции. E владеет подписками и видимостью пользовательских данных; доставку запускает G из устойчивого outbox.

## F — каталог, рекомендации, SEO, реклама (tor-search,tor-recommendations,tor-referrals)

| Метод+путь | Данные | Правило |
| --- | --- | --- |
| GET `/api/v1/books` | **сохранить массив из bokarust**, `limit=1..100` | id числовой только для старой совместимости, default20; поле body отсутствует |
| GET `/api/v1/search/books?q&genre&language&after&limit` | `{items:[{id,title,language,cover_url?,rating?}],next_cursor}` | keyset, limit1..100, недоступные книги отфильтрованы **до** limit |
| GET `/api/v1/books/{id}/recommendations` | `{items,next_cursor}` | нет приватных текстов/кешей, пустая выдача допустима |
| GET `/api/v1/reading-universe?scope=global|book|user&after` | bounded feed | только опубликованные и доступные метаданные |
| POST `/api/v1/me/rankings/choice` | `{left_book_id,right_book_id,winner_id?,defer?}` | defer/resume, повтор не удваивает рейтинг |
| GET `/sitemap.xml`, `/robots.txt` | canonical URLs | draft/deleted/hold не индексируются, staged generation и rollback |
| GET/POST `/api/v1/me/referrals` | invitation token, attribution | self-referral запрещён, деньги начисляет D |
| GET/POST `/api/v1/ads/campaigns` | campaign draft/budget/targeting | write только рекламодателя-владельца; delivery=false без verified provider |
| GET `/api/v1/me/analytics` | агрегаты | персональные данные и текст главы не логируются |

Рекламные начисления и выплаты — D, а не F. Действующие в NEwBOO `chapter_ads` rollout flags были выключены; не выдавать UI черновой кампании за реальную монетизацию.

## G — импорт, изображения, очередь (tor-imports,tor-ingest)

| Метод+путь | Тело/ответ | Важная проверка |
| --- | --- | --- |
| POST `/api/v1/import-jobs` | `{book_id?,source_url?,mode:"url|upload"}` → `{job_id,state:"queued"}` | actor permission, safe URL/DNS/redirect; не делать прямую публикацию |
| POST `/api/v1/import-jobs/{id}/upload` | bounded multipart ≤32 МиБ | archive limits, MIME/magic, streaming, временное хранилище |
| GET `/api/v1/import-jobs/{id}` | `{job_id,state,completed,total,failures,checkpoint}` | владелец, после рестарта сохраняется статус |
| POST `/api/v1/import-jobs/{id}/cancel` | `{expected_revision}` | CAS; отменённый worker не продолжает писать |
| POST `/api/v1/books/{id}/cover` | image ≤8 МиБ, пиксели ≤24MP | content-type не доверять, object hash, metadata stripping |
| GET `/api/v1/me/notifications` | bounded inbox | idempotent event delivery, preferences opt-out |
| GET `/api/v1/admin/jobs` | состояния queued/running/dead | admin-only; не создавать несуществующие в NEwBOO операции retry/cancel без нового контракта |

Порядок: job → parser → preview → **B API command** create/update chapter → B publish event в outbox → G delivery → F search / E notification. G не пишет в `chapter_versions` напрямую.

## H/I — HTML, интеграция и окружение

- H сохраняет серверный HTML с progressive enhancement, один server-rendered shell. В `web` нет SQL, security decision или бизнес-транзакции. Тест каждой формы на реальном Rust сервере+PG и двух ролях.
- I добавляет `GET /readyz`: 200 лишь после `SELECT 1` и миграции; `GET /healthz` остаётся `ok` процесса. Внутренний `/metrics` только на закрытом admin/loopback доступе, с redaction.
- I записывает один автоматизированный контрактный тест для каждого метода/пути: ответ на success, отсутствие права, schema/размер, idempotency где применимо. Исполнитель A–G реализует его в собственном `tests/<slice>_http.rs`, не переносит все в общий God-suite.
- SSR fallback `/login` и `/register` в S0 demo **не считают работающими**: до подключения PG эти страницы информируют об отключённых формах и не принимают пароль.

## Обязательные контрольные запросы

```text
GET /healthz                                      -> 200 'ok' при живом процессе
GET /readyz                                       -> 503 без PG, 200 после миграций
GET /api/v1/books?limit=101                       -> 400 (историческое поведение bokarust)
GET /api/v1/books                                 -> JSON array (не envelope)
GET /reader/<hidden-or-unpublished-chapter>       -> 404 + 0 байт скрытого markdown
POST /chapters/<paid>/buy with same command_key    -> одна запись ledger
GET /api/v1/me/library without session            -> 401 + no-store
PUT /api/v1/me/progress/purchased for revoked buy  -> отказ + ни одной записи позиции
POST /api/v1/import-jobs to http://127.0.0.1       -> 422 + никакого соединения к localhost
```

В CI проверять контракт на текущем SHA. Если реализация расходится с принятым контрактом, **исправлять код**, а не задним числом делать тест слабее.
