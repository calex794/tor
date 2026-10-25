# TOR — RIL и сигналы чтения при прямой доставке S3
> **Область этого дополнения:** события чтения и рекомендаций — только один потребитель [общесистемной RIL-платформы](ril-platform-v1.md). Назначение RIL определяется независимыми Route Intelligence, Behavior Intelligence и Route Governance, а НЕ S3 или рекомендациями. Подробности транспорта главы здесь — частная интеграция, не архитектурный центр RIL.



**Статус: подготовительный контракт, НЕ работающая реализация.** Дополнение к [рекомендательному роадмапу](../roadmap/recommendations-execution-contract.md) и [S3-архитектуре](https://telawiki.com/spaces/430/pages/7736/tor-s3). НЕ десятая линия, НЕ новый независимый реестр задач.

Исходный код NEwBOO: [Route Intelligence Layer](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/route_intelligence_service.py), [RIL middleware](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/main.py#L1584-L1649), [recommendation hygiene](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/recommendations/services/signal_hygiene_service.py), [signal weighting](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/recommendations/services/signal_weighting_service.py), [trusted reader completion](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/reader_progress_service.py#L303-L449), [completion table](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/db/migrations/ydb/1256_reader_chapter_completions.sql).

## 1. Четыре независимых контура

1. **RIL:** классифицирует HTTP-запрос (маршрут, семейство, поверхность, область продукта, стоимость, риск) и отдаёт bounded паспорт для остальных модулей. Сам НЕ определяет покупку и НЕ доказывает чтение.
2. **Business events:** совершённая покупка, регистрация книги в библиотеке, подтверждённый прогресс, завершение главы — результат доменной транзакции PostgreSQL.
3. **Recommendation signals:** прошедшие RIL/path hygiene, авторизацию, dedup и версионные правила проекции событий; вес + generation находятся у F, не в RIL.
4. **S3 telemetry:** объектные GET, исходящие байты, хранение, ошибки и цена. Это НЕ поведенческие сигналы.

**Новый путь платной главы:** Rust API (D entitlement) выдаёт временный signed GET и scoped version DEK → браузер скачивает зашифрованный файл напрямую из S3 → WebCrypto показывает текст → браузер отправляет validated progress в Rust/PostgreSQL. **S3 GET не попадает в Axum RIL**. GET metadata, POST grant, PUT progress, покупка, клики по каталогу и рекомендациям попадают.

**Запрет:** grant issued, ciphertext GET, prefetch или decrypt сами по себе не равны читательскому завершению. Длительное чтение без HTTP после S3-загрузки — нормальная последовательность, не сигнал ухода/сканирования.

## 2. Контракт технического паспорта RIL

RIL использует версионированный RoutePassportV1: route_known (bool), route_family (ограниченный словарь без slug/book/user ID), route_match_kind (exact/prefix/fallback/unknown), route_decision (allow/diagnostics/allowlisted/deny), traffic_surface (public_page/api/internal/admin/static/service/unknown), product_area (reading/catalog/auth/billing/recommendations/advertising/storage/...), cost_class (free/cheap_read/expensive_read/write/background/unknown), risk_hint (normal/suspicious/scanner/unknown), версии taxonomy/classifier/known-route.

**I в Axum:** immutable taxonomy/route pattern registry при инициализации; на запросе классификация по HTTP method и framework MatchedPath, без сканирования всего роутера и без SQL. Политика отказа/anti-bot/rate-limit — отдельный потребитель A/I, не часть классификатора. Не вставлять raw URL, IP, UserId, session, S3 signed query, chapter-id и email в RIL labels/trace/log. Служебный request-id может находиться в приватном trace, но не в low-cardinality metrics labels.

**Новые известные семейства:** GET reader metadata, POST reader delivery-grants, POST paid asset-grants, PUT progress/free, PUT progress/purchased, POST purchases, POST upload prepare/finalize, GET public chapter. Их RIL должен идентифицировать как normal reading/catalog/billing/storage, а НЕ как unknown scanner. Сама прямая загрузка S3 GET не является маршрутом TOR.

## 3. Исходные рекомендательные события и новые источники

| Event | Исторический вес NEwBOO | Допустимый источник в TOR |
|---|---:|---|
| book_opened | 100 | Действительный разрешённый запрос карточки |
| catalog_clicked/search_clicked | 150 | Атрибуция фактического клика |
| reader_opened | 250 | Авторизованное открытие ридера (не просто grant) |
| chapter_started | 300 | Проверенная первая осмысленная позиция прогресса |
| chapter_finished | 700 | Серверно подтверждённый последний читаемый блок, атомарное событие |
| library_added/bookmark_added | 800 | Действительное изменение записи библиотеки/закладки |
| purchase_completed | 1000 | Только committed ledger + entitlement в D |
| book_finished | 1000 | Отдельная подтверждённая агрегация завершений |

Эти веса — фиксированные старые значения Python, НЕ показатель качества. Менять через явную algorithm_version и golden fixtures. **Нет события chapter_finished из произвольного клиентского сообщения или запроса signed URL**. Технический reader_loaded после браузерной расшифровки может быть weak diagnostics, но не trusted completion.

## 4. ReaderVersionManifest — чтобы сохранять прогресс без скачивания S3 в backend

NEwBOO server проверял точную доступную версию, существование block_id и достижение последнего читаемого p-/img- блока. Если TOR хранит тело только в зашифрованном S3, нельзя на каждый PUT progress скачивать и дешифровать весь текст.

**B при сохранении/публикации создаёт immutable ReaderVersionManifestV1 в PostgreSQL** с chapter_id, chapter_version_id, content_hash, parser_schema_version, стабильными логическими block_ids, last_readable_block_id и количеством блоков. Никакого текста, личной информации или signed URL. Блоки независимы от viewport/ширины экрана; идентификаторы относятся к точной версии. Manifest отдаётся C через typed contract, а не прямо публичному анониму.

**C для PUT progress/free или progress/purchased:**
1. Проверить Actor, D entitlement (включая pinned V1), версию и ACL, monotonic revision/CAS.
2. Проверить block_id по manifest точной версии; неверный блок/версия/refund → отказ без изменения прогресса и без сигнала.
3. Средний блок → сохранить позицию, никакого завершения.
4. Финальный readable block и согласованный порог offset/progress → в ОДНОЙ PG транзакции обновить прогресс, записать факт reader_chapter_completions и outbox ChapterCompleted. Unique (user_id,book_id,chapter_id) по эталону NEwBOO: первое завершение V1 фиксируется один раз, повтор/retry/V2 не генерирует второе событие. EventId устойчивый и серверный.
5. G доставляет durable outbox F; F записывает chapter_finished ровно один раз по EventId. Не делать вид, что структурный скролл — неопровержимое доказательство человеческого чтения; для fraud/meaningful_read использовать дополнительные осторожные сигналы, если согласованы.

Это позволяет **и сохранить рекомендации, и не гонять зашифрованную главу через домашний Rust ради каждого прогресса**.

## 5. Рекомендации, hygiene и нулевая аудитория

RIL наблюдает последовательность catalogue → book → reader → grant → progress; сетевого TOR-запроса между grant/progress может не быть, и это нормально. Проводить Route/BehaviorPathHygiene отдельно от регистрации бизнес-события: scanner/unknown/degraded запрещают training-сигнал, watch понижает вес, normal допускает. Не превращать технические GET грантов в популярность/атрибуцию. Ограничение частоты grant — защита денег S3 от злоупотребления, не критерий полезности книги.

F строит read models Recent/Behavioral/Content/Popularity/Related, дедуплицирует события, проверяет текущую видимость книги при отдаче. При НУЛЕ пользователей — deterministic cold-start по публичным метаданным/контенту, а не фальшивые чтения. Профили пользователя/гостя — минимум данных, сроки хранения, opt-out и отдельное согласование политики приватности. Коммерческие показы рекламы не равны органической рекомендации.

## 6. Владельцы и приёмка

**I:** shared passport/versioned Axum middleware, маршруты grant/progress, трассировка, taxonomy tests, общие контракты и CI. **B:** immutable структурный manifest версии. **C:** права+progress+server-trusted completion. **D:** purchase event только после committed ledger/entitlement. **F:** source event hygiene, weighting, behavioral graphs и dedup/generations. **G:** durable outbox, consumers, replay и cost telemetry. **H:** WebCrypto reader, bounded progress/flush/retry, browser e2e. Это расширение уже существующих I-02/I-13, B-04, C-04/C-06/C-09, D-02, F-07/F-12, G-06/G-07, H-05/H-12.

**Обязательная PG + Axum + реальный private S3 + Chromium приёмка:**
- Два пользователя: U1 purchased V1, U2 no purchase; сеть показывает S3 → браузер; U2 не может отправить paid progress.
- Grant и успешный S3 GET сами не создают chapter_finished. Mid progress — тоже нет.
- Корректный финальный блок/threshold → ровно один ChapterCompleted + один recommendation chapter_finished; два устройства/retry/V2 не удваивают.
- Refunded, wrong block ID/version, сканерские запросы, неизвестные маршруты — без положительного recommendation signal.
- RIL правильно классифицирует все новые /reader/... маршруты, не маркируя легальный paid grant как unknown.
- Сбои PG/G/outbox и S3, частичная доставка/повтор, cold start, private metadata и fraud; безопасный retry без потери event.
- В логах/метриках нет paid text, raw DEK/KEK, signed URLs, raw path/PII как labels. На финальном SHA — CI/PG/browser и запись в существующую Notion-матрицу.

**Факт исполнения:** пока документ и план, не реализованный код.

## 8. Важный async-boundary: контекст RIL переживает отложенную доставку

В исходном NEwBOO рекомендационный hygiene отвергает missing_context; поэтому **нельзя** в G-worker пытаться заново классифицировать HTTP-запрос, которого больше нет. C/D/F в момент авторизованной исходной бизнес-транзакции берут bounded RoutePassportV1 из контекста текущего request и записывают его **с версией и provenance** в outbox/event envelope (или ставят контролируемый provenance=trusted_internal для настоящего backend/system event). В PG могут храниться необходимые user/book/version IDs для прав, но **не** raw URL, S3 GET link, DEK, IP/UserAgent в RIL passport и high-cardinality labels.

F при обработке задержанного/повторённого события читает сохранённые passport/provenance, а не вычисляет его по маршруту worker; разрешает server_committed ChapterCompleted только из typed авторитетного outbox и не доверяет строке client-supplied trusted_completion=true. При missing/unknown provenance не превращать событие в normal по умолчанию: fail closed в рекомендациях, но не терять факт прогресса/покупки из PG. При retry сохранять исходный event_id, паспорт/версию taxonomy и исходную временную метку. Тест: G-worker после рестарта/через час принимает сохранённое событие ChapterCompleted один раз; клиентская имитация того же payload не принимается.
