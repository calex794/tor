# TOR Events & Analytics v1 — доверенные факты вместо имитации чтения (PREF-06)

**Статус:** выбранный каталог доменных событий, ограничений хранения и доверенных метрик для F/G/C/D/E/I. Источник [RIL+S3](ril-reader-signals-v1.md), [NEwBOO reader_progress_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/reader_progress_service.py), [rec golden](recommendations-golden-v1.md), [реклама](finance-ad-exact-v1.md). **Покупка, загрузка из S3 и показ страницы — три разных действия; ни одно само по себе не доказывает чтение.**

## 1. Единый `BusinessEventV1` и владелец записи

Envelope содержит `event_id(UUID),event_type,event_version=1,occurred_at_utc,actor_provenance,source_commit_authority,resource_ref,correlation_id,route_passport_v1?/trusted_internal,privacy_class,payload`. Значение `actor_provenance` ставит **сервер** в момент PG commit; пользовательское `trusted_internal=true` игнорируется/422. В `route_passport` — bounded категории RIL, никакого raw URL/query/IP/DEK/UserAgent. Доставка через [outbox, required_consumers](../roadmap/data-and-events.md); каждый consumer использует `(consumer,event_id)`, доставка ≥at-least-once, доменные эффекты idempotent.

| Код события | Единственный authoritative owner | Когда записать | Потребитель/что НЕ значит |
| --- | --- | --- | --- |
| `BookPublishedV1` | B | После атомарного commit новой публичной редакции/книги; V1→V2 отдельно по version | F index/SEO, E follower notifications; не “read” |
| `BookOpenedV1` | F/H middleware | Реально разрешённый, успешный book detail resolve+200 | F слабый интерес; 200 metadata ≠ paid reader |
| `CatalogClickedV1` | F/H | Signed attribution у canonical book-open, не произвольный POST click | F атрибуция; crawler GET игнорировать |
| `ReaderOpenedV1` | C | Авторизованный успешный entry в viewer UI, не выдача/повтор S3 grant | F слабый сигнал, не completion |
| `ChapterStartedV1` | C | Первый успешный PG progression/CAS на валидированном читаемом блоке неизменяемой версии | F intermediate activity, не 100% human attention |
| `ChapterCompletedV1` | C | Проверен **последний читаемый block_id** из `ReaderVersionManifestV1` B, версия+entitlement valid, один PG update progress+unique completion+outbox | F `chapter_finished`, E achievements; не вывод из S3 GET |
| `BookCompletedV1` | C | Отдельная серверная агрегация всех релевантных завершений по опубликованной/купленной книге | F вес 1000; часть глав может быть hidden и требует policy |
| `LibraryAddedV1` | C | Реально committed запись личной библиотеки | F weight 800; не paid entitlement |
| `PurchaseCommittedV1` | D | Ledger+purchase+entitlement в одном PG commit | F weight 1000, creator earnings; не read |
| `RefundCommittedV1` | D | Canonical reversal/entitlement revoke | F stop positive retention, D ledger, E уведомления |
| `AdViewableConfirmedV1` | F/Ads | ≥60% видимости карточки ≥1 сек + server-verified signed assignment/session/effect | D reservation; **не organic rec, не pageview** |
| `AdImpressionBilledV1` | D/Ads posting port | Accepted dedup+hard cap+reservation commit | Advertiser stats; не click/conversion |
| `ContentHiddenV1` | B/E | Модерация/status/withdrawal applied | F/G invalidate projections+S3 public URLs |
| `NotificationDeliveryV1` | G | Inbox/email after verified consumer ack | delivery ≠ read; нельзя выводить email-open из send |

**S3 GET**/bytes/transfer/errors/egress — исключительно `StorageTelemetryV1` с bucket class/bytes/status, **не** reader business event. POST paid grant отражает authorization/technical cost, не создаёт `ReaderOpened` или `ChapterCompleted`.

## 2. ReaderVersionManifest и необманывающий «meaningful read»

B при публикации готовит immutable manifest `(chapter_id,version_id,content_hash,parser_schema_version,blocks:[stable_id,kind,ordinal],last_readable_block_id)`. Не сохранять текст в manifest, не доверять client-chosen `last_block_id`. При PUT progress:
1. C проверяет Actor+entitlement D+exact version/current/Layer term+CAS, membership не приравнивает к Purchase.
2. `block_id` содержится в manifest, порядок не откатывается по stale revision; изменение offset в пределах блока валидируется; refunded/hidden/deleted/blocked/future version denied.
3. Первый подтверждённый прогресс → `ChapterStartedV1` idempotent per reader/chapter/version, повтор/устаревший revision не создаёт событие.
4. Последний читаемый блок + корректный offset/manifest threshold → **в одной PG транзакции** `reader_chapter_completions UNIQUE(user_id,book_id,chapter_id)` и outbox `ChapterCompletedV1`. Повтор, второе устройство или V2 после уже подтверждённого завершения дают **0 дополнительных** `ChapterCompleted`, хотя latest progress может иметь другую версию.
5. G через час/restart передаёт исходный сохранённый RIL паспорт и event id, F не пытается классифицировать путь worker. Missing provenance не повышать до trusted.

**Окончательное правило для первой версии:** метрика `meaningful_read_v1` = **первое серверно подтверждённое ChapterCompletedV1**. Это строгий нижний предел «серьёзного чтения», а не статистическая оценка времени, и не гарантирует, что человек лично прочитал весь текст. Частичное чтение показывать отдельной метрикой `validated_progress_count`, без именования «дочитал/полноценно прочёл». Расширение до времени/нескольких блоков — только новый `metric_version=v2` после отдельных тестов на накрутку. Никакой неизвестный «длительный GET» не становится meaningful read.

## 3. Классификация маршрутов и рекомендательная нагрузка

RIL типы `RoutePassportV1`: known, route family, match kind, product area, surface, cost class, risk hint, classifier version. I берёт Axum `MatchedPath` на исходном HTTP запросе; только bounded категории. `POST /api/v1/reader/chapters/{id}/delivery-grants` и progress — известные, не scanner; S3 GET отсутствует и не обязан появляться в Axum трассе.

Python веса для F: book_open=100; catalog/search click=150; reader_open=250; chapter_started=300; chapter_finished=700; library/bookmark=800; purchase=1000; book_finished=1000. Не смешивать веса разных событий одного `event_id`, не выдавать повышенный рейтинг из зацикленного heartbeat. `hygiene=degraded/scanner/unknown` → **0 обучающих сигналов**, но **доменный факт покупки/прогресса остаётся в PostgreSQL**; не удалять деньги из-за антибота.

## 4. Приватность и хранение — дефолтные безопасные параметры

**Выбраны как технические CONFIG по минимуму данных, не как юридическое утверждение о сроке хранения:**
- сырой нефинансовый `reader_activity/recommendation_attribution` c псевдонимным subject token: **не более 30 дней**, TTL/partition prune, event_id dedup на всё окно; хранить `chapter_id`/`book_id` для агрегатов, но без текста/запросов/Email/raw IP/полных path.
- агрегаты per book/day или per campaign/day, очищенные от PII, — **до 180 дней** (дальнейший срок требует согласованной политики); age/access filtering на выдаче. Малые сегменты пользователей/друзей не публиковать как персональные факты.
- канонические purchase/ledger/refund/payout и trust/provenance receipts — **не попадать под 30-дневный GC**; их сохранность, удержание и удаление определяются отдельной денежной/правовой политикой, поэтому irreversible purge запрещён до подтверждения владельца.
- opt-out/delete personal recommendations: остановить новые персонифицированные сигналы и удалить/анонимизировать derivative profile/edges в пределах окна, сохраняя отдельно обязательные финансовые и security facts по указанному законному основанию; надёжное стирание кешей проверять тестами.
- **Логи/метрики:** только low-cardinality `event_type,route_family,status,algorithm_version,campaign_state`. Никогда raw `user_id/email/IP/reader book-body/signed S3 URL/DEK/KEK` как label; sampled трасса request_id хранится в защищённом журнале доступа.

В производственном выпуске нужна проверка локальных правил защиты персональных данных и согласия на поведенческую персонализацию; пока такого доказательства нет, `personalized_recommendations_enabled=false`, безопасный публичный fallback доступен. Внешние рекламные cookies/идентификаторы не добавлять по умолчанию.

## 5. Таблицы/уникальность

```sql
CREATE TABLE reader_chapter_completions (
 user_id BIGINT NOT NULL, book_id BIGINT NOT NULL, chapter_id BIGINT NOT NULL,
 first_version_id BIGINT NOT NULL, completed_at TIMESTAMPTZ NOT NULL,
 source_event_id UUID NOT NULL UNIQUE,
 PRIMARY KEY(user_id,book_id,chapter_id)
);
CREATE TABLE business_event_receipts (
 consumer TEXT NOT NULL, event_id UUID NOT NULL,
 processed_at TIMESTAMPTZ NOT NULL,
 PRIMARY KEY(consumer,event_id)
);
CREATE TABLE reader_event_projection (
 event_id UUID PRIMARY KEY, subject_token BYTEA, event_type TEXT NOT NULL,
 book_id BIGINT NOT NULL, chapter_id BIGINT, event_time TIMESTAMPTZ NOT NULL,
 route_provenance_version INTEGER NOT NULL, effective_weight INTEGER NOT NULL
);
CREATE INDEX reader_event_projection_expiry ON reader_event_projection(event_time);
```

Таблица receipt может использовать уже существующую `event_deliveries` без дублирования — интегратор I **не** создаёт вторую таблицу, если выполняется `PRIMARY KEY(consumer,event_id)`. G делает 100 recipients/batch cursor delivery; ни 101-й, ни 201-й подписчик не теряется.

## 6. Приёмочные тесты EVT-01…12

| ID | Опыт | Ожидаемое |
| --- | --- | --- |
| EVT-01 | только paid grant/S3 GET без прогресса | 0 started/completed/meaningful |
| EVT-02 | один mid manifest block/повтор | 1 started, 0 completed |
| EVT-03 | валидный final block + CAS | 1 completed +1 meaningful |
| EVT-04 | U1 два устройства, старый revision, опубликована V2 | no duplicate completion, progress monotonic |
| EVT-05 | чужая, refunded, 18+/blocked версия | deny, 0 trusted signals |
| EVT-06 | G crash до второго consumer ack | first consumer не повторяется, eventual second ack |
| EVT-07 | C отправил trusted event через час после HTTP | RIL provenance persisted, no worker-path reclassification |
| EVT-08 | forged client trusted flag/сканер/unknown route | 0 eligible recommendation signals |
| EVT-09 | publish→hide при старой recommendation generation | hidden book отсутствует, сигнал не раскрыт |
| EVT-10 | expired retention/opt-out | raw privacy events removed; финансовые facts сохранены |
| EVT-11 | ad 59%/0.9s и 60%/1s | только последний accepted и финансово резервирован |
| EVT-12 | trial 201 notifications + restart | все корректные получатели видят только один Inbox, retry без дубля |

**Выход исполнителя:** фактические PostgreSQL migration + typed schema + Env CONFIG, unit/PG/Chromium tests, bounded RIL metrics и матрица доказательства на конечном SHA. Письменный документ и JSON тестовые оракулы сами события не создают.
