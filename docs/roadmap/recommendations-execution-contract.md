# TOR — рекомендации, поисковые проекции и «Вселенная чтения»: инструкция
> **Архитектурная граница:** рекомендации — потребитель независимого RIL, а не владелец или определение его назначения. Базовая [спецификация Route Intelligence / Behavior / Governance](../contracts/ril-platform-v1.md), этот документ описывает только F-потребителя. Нельзя сокращать перенос RIL до recommendation signal hygiene.



**Связанные функции:** F010, F033, F034, F035, F047, F056; S008, S044, S045, S046, S066. Текущий TOR F-05/F-06/F-07/F-12/F-13 — **только родительские задачи**. Старый [NEwBOO recommendations](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations) содержит **53 отдельных service-модуля**; простое `sort popularity` не восстанавливает его поведение. Достоверные опоры: [hybrid_scoring_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/services/hybrid_scoring_service.py), [recommendation_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/services/recommendation_service.py), [runtime_config.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/runtime_config.py), [materialization_orchestrator_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/services/materialization_orchestrator_service.py).

**Граница:** рекомендации = построить кандидатов → вычислить интерес → отфильтровать доступ → вывести результат. Рекламные карточки и их оплаченные показы — **отдельный контур ads**, даже если UI использует те же metadata карточки; нельзя тихо пропускать коммерческий score через organic search/recommendation и выдавать это за естественный рейтинг.

## 1. Контракт выдачи

**Пять поверхностей** существующего Python runtime: `home`, `book_detail`, `reader_end`, `catalog`, `library`. Для каждого ответ:
```json
{"items":[{"book_id":"123","reason":"similar_by_content","algorithm_version":"recommendations_v1","source":"content"}],"generation_id":"g123","next_cursor":null}
```
Никакого body/платной цитаты/скрытых author заметок, user_id другого субъекта, сырых сигналов. Публичный viewer получает только разрешённые рекомендации, авторизованный — только metadata eligible для **его** возрастных и приватных настроек. Filter visibility/book hold/deletion и доступ по текущим B/A policy выполнить **после подготовки candidates и перед сборкой последнего response**, а не считать «очищенную проекцию» вечно разрешённой.

**Источник списка ID:** публичные ID B, ключ `BookId` из I, контракты одного typed `RecommendationMetadataProvider`; тяжелые измерения в PG read-models F; query budget ограничен и измеряется, прямой просмотр таблиц A/D запрещён.

## 2. Алгоритм: заданные правила из исходного кода

| Подсистема | Принятый алгоритм | Границы/ошибка |
| --- | --- | --- |
| Источники кандидатов | `behavioral` (соседи по последовательностям чтения), `content` (темы/жанры/автор/язык), `popularity` (нормализованная популярность), `related_books` (явно связанные книги) | один provenance на каждый source, дубликат по book/source схлопывать |
| Рекомендательные профили | user recent, явный парный рейтинг, age/genre preferences, session; user может не иметь профиля | аноним/новичок — проверенный popularity fallback |
| Hybrid | каждый source candidate normalizes to bounded 0..1000; для `behavioral/content/popularity` веса целые ≥0 **в сумме 100**; доля = `normalized*weight//100` | нельзя выдавать кандидатов с отсутствующей нормализацией; input max 500 |
| Dedicated `related_books` | вручную связанные книги идут отдельным deterministic каналом с приоритетом, не получают чужой source weight и не удваивают same BookId | access/visibility hard gate, no duplicate output |
| Разрешение ties | deterministic reason/source order и BookId, все tie rules повторяют код Python | два прогона одной генерации → тот же порядок |
| Quality signal | bounded post-weight boost, в старой модели максимум **50** к bounded итоговому score; итог ≤1000 | подозрительные/неподтверждённые quality events не улучшат score |
| Ограничение популярности | Python hybrid `max_popularity_results=10`, max output=50 | нельзя забить всю выдачу одной популярностью |
| Serving бюджет | `recent_books_limit=5`, `behavioral/content_limit_per_source=20`, `related_books=10`, `popularity/fallback=20`, `metadata=100`, `response=20` | не делать unbounded N+1/live joins в HTTP |
| Возраст/доступ | B/A policy на каждом response, без signed paid URL и без скрытого related hint | ново-заблокированная книга исчезает и из старой generation |
| Деградация | при unhealthy generation/DB timeout — bounded safe fallback public list или empty, не старый приватный cache | fail-closed по доступу, явный warning metric |

**Весовые профили** не выбирать наугад «40/30/30». Сохранить детерминированное поведение Python `scenario_detection_service.py` / `signal_weighting_service.py` в golden fixtures с одинаковым source в одном SHA; если изменяем продуктовый вес — это отдельный явно утверждённый алгоритм v2, а не молчаливая догадка Rust.

## 3. Вспомогательные проекции и обновление поколений

- `rec_source_signals`: обезличенный subject token, book_id, event_kind, event_time, event_id для dedup; подтверждённый read/complete отдельно от pageview.
- `rec_behavioral_edges`: source_book_id → candidate_book_id, bounded score и окно; не хранить произвольные пользовательские последовательности.
- `rec_content_edges` и `rec_popularity_snapshots`: generation_id/book_id/score/reason, ключ и индекс по поколению.
- `rec_generation_manifest`: generation_id, algo_version, built_at, source_windows, statuses `building/validated/active/rejected`; одна `rec_active_generation` row/pointer. Поколения immutable после validation.
- `rec_attribution_events`, `rec_quality_daily`: событие показа/открытия/более глубокого чтения отдельно; idempotent event IDs, пересчёт из раздельных причинных фактов.
- `reading_universe_items` и bounded materialization generation для глобальной, book и user лент. Не превращать это в очередной персональный feed на каждый HTTP request.
- `recommendation_experiment_assignment` не требует массовой персистенции raw identity: stable hash(subject,experiment_key) → deterministic arm/holdout, отражение в диагностике без раскрытия субъекта.

F владеет проекциями/score; G запускает и повторяет jobs, не становится authoritative владельцем рекомендаций. Каждое новое поколение строится в собственной области и **не активно до quality gate**, failed build не стирает предыдущую рабочую выдачу. В production runtime запрещено на request запускать тяжёлый full materialization.

## 4. Развёртывание и качество

Старый Python имеет `RecommendationProductionRollout` ступенями **1→5→10→25→50→100%** с `hold`, `rollback` и health/quality/experiment gates; перенести механизм как feature flags/версионированную политику, не делать «включили 100% по умолчанию». Нужен заранее сформированный контрольный набор со сценариями:
- новый пользователь, активный пользователь с историей, только одна прочитанная книга, пустой каталог, иностранный язык/жанр;
- внезапно скрытая/blocked/deleted/18+ книга и редактирование source metadata между generation и GET;
- одинаковая книга из 3 источников + tie, score≤1000, weight sum=100, quality≤50;
- интересы пользователя vs advertiser: оплаченный показ не подменяет organic recommended reason;
- 100k books и измеримый bounded query count/latency (не искусственно требовать нереалистичный p95 без стенда);
- два job/restart/crash во время generation, stale pointer/corrupted manifest, backfill после outage;
- holdout/experiment split, статистическое качество, отсутствие накрутки quality signal от простого показанного HTML;
- social proof friend/block/opt-out: не раскрывать, кто читал закрытый материал.

**Безопасная метрика meaningful_read** ещё не полностью привязана к пользовательским событиям: page view/открытие книги ≠ подтверждённое чтение, purchase ≠ completion. Это отдельное предметное соглашение на F056, см. `preflight-audit-2026-10-26.md`. Сбор статистики должен хранить reason для отказа и aggregate counts, без текста глав/IP/email.

## 5. Пакеты реализации

| ID | Владение/результат | Действие | Проверка |
| --- | --- | --- | --- |
| REC-01 | I+F `contracts/src/recommendations.rs` | typed CandidateSource/Reason/AlgorithmVersion/GenerationId, safe metadata provider, пятёрка surfaces | JSON+unit serde, no direct cross-feature dependency |
| REC-02 | F `recommendations/src/repositories/signals.rs` | нормализация входных событий, privacy/dedup, UTC bounded windows | 2 одинаковых read events = 1 signal |
| REC-03 | F `services/behavioral_candidates.rs` | bounded top edges, переходы reading/readers-also-read | пустое/битое окно, 50k transitions max, no invalid subject leak |
| REC-04 | F `services/content_candidates.rs` | similarity tags/genres/author/language из текущего B metadata snapshot | private/draft book never candidate final |
| REC-05 | F `services/popularity_candidates.rs` | trending/popular/new/fallback с нормализацией для cold start | empty user получает выдачу; 100k books bounded PG |
| REC-06 | F `services/related_books.rs` | ручные book relations как отдельный dedicated source | related не дублирует hybrid, source unavailable gracefully |
| REC-07 | F `services/hybrid_scoring.rs` | перенос normalization, scenario weights, dedup, tie, max candidates/boost/caps | Python golden fixtures deterministic; score≤1000 |
| REC-08 | F `services/preference_ranking.rs` | user pairwise choice+defer/exhausted/resume в отдельном user profile | 2 устройства, stale pair, block/hidden -> not asked |
| REC-09 | F `services/generations.rs` | PG materialization generation + pointer activate/reject + bounded cursor jobs | crash between build/activate safe, old generation preserved |
| REC-10 | F `services/assembly.rs` | пять surfaces, access/filter before response, bounded metadata, safe fallback | 2 users/guest, blocked/18+/paid metadata, no N+1 |
| REC-11 | G + F `services/rec_refresh.rs` | событие B/E/read → bounded incremental invalidation, backfill/compaction | не теряется событие при worker crash, no duplicate projection |
| REC-12 | F `services/experiments.rs` | experiment/holdout, staged rollout 1/5/10/25/50/100, automatic hold/rollback by health | bad generation/latency forces rollback |
| REC-13 | F `services/attribution.rs` | независимый fact impression→book_open→read→library/purchase, раздельные metrics, anti-fraud | просмотр HTML не считается meaningful_read |
| REC-14 | H + I `web/pages/recommendations.rs` | реальные страницы home/detail/reader_end/catalog/library и PG/Chromium сравнение NEwBOO | site works with rec disabled/fallback/active, one SHA and 100k-case query plan |
 
**Критерий готовности продукта:** R01–R14 на реальной PostgreSQL+browser, active generation действительно возвращает разрешённые книги с объяснением, cold start не пуст без оснований, тесты JS/request/traffic показывают реальную скорость. Без них S044/045 «в плане» не равно рекомендациям NEwBOO.

**Интеграция с рекламой:** публичные candidate IDs/metadata общие, но рекламный reserve/escrow/impression в F-ad/D, а organic scoring в F-recommendations; смешение без явной маркировки/consent нарушает раздельный финансовый и рекомендательный контракт.


## 6. RIL и прямое S3-чтение — обязательный источник качества сигналов

**Норматив:** [RIL+reader signals v1](../contracts/ril-reader-signals-v1.md) и [paid S3, Tela](https://telawiki.com/spaces/430/pages/7736/tor-s3). Источник NEwBOO: [RIL classification](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/route_intelligence_service.py), [recommendation hygiene](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/recommendations/services/signal_hygiene_service.py), [trusted completion](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/reader_progress_service.py). **RIL — не рекомендательный движок, а техконтекст доверия к сигналам; рекомендательная модель потребляет отдельные события чтения/покупок/кликов.**

- При прямом чтении paid ciphertext **S3 → браузер** backend RIL **не видит GET к S3**. При этом он видит GET reader metadata, POST выдачи подписанного доступа и PUT progress. **Grant, S3 GET, cache/prefetch не считаются reader_opened, chapter_finished или meaningful_read**.
- Ввести на стороне I bounded и versioned RoutePassport: family/surface/area/cost/risk/known/match/decision; известные правила для POST paid-grant и PUT прогресса; политики запрета/сканера отдельно от RIL. Не использовать raw path/ID/signed URL как metrics labels.
- **Важное изменение для S3:** B при публикации генерирует immutable **структурный ReaderVersionManifest** (logical block IDs, last readable block, hash/schema_version), который C читает из PostgreSQL. При сохранении прогресса C проверяет право на точную pinned версию и достижение последнего блока по manifest, а не скачивает тело платной главы из S3 на каждый heartbeat. По факту первого завершения C атомарно записывает completion+outbox; unique (user_id,book_id,chapter_id) согласно NEwBOO (повтор и V2 не делают второй completion). G доставляет F событие ровно по EventId, F создаёт weighted chapter_finished.
- F проверяет RIL/behavior-path hygiene до использования recommendation source event; unknown/scanner/degraded не обучают, watch может ослаблять сигнал. Отдельные произошедшие события не заменять HTTP counters; committed purchase подтверждает покупку, но не факт прочтения.
- **Cold start = 0 пользователей:** не генерировать фальшивую историю, выдавать только разрешённый content/popularity/public metadata fallback, затем строить поколение по реальным событиям. Paid content/hidden metadata не должны утечь из source events и результатов.
- Новые обязательные отрицательные e2e: legit grant classified as reading, S3 GET не появляется в RIL и не обязан; successful grant без read-progress → **0 completion**; last block → 1 completion+1 rec signal; repeated/retried/second device/second version → не больше одного; refund/wrong version/wrong block/unknown/scanner → 0 positive; G crash after PG commit → eventual one result; no signed URL/DEK/raw ID in RIL labels.

Подшаги этой секции относятся к существующим I-02/I-13, B-04, C-04/C-06/C-09, D-02, F-07/F-12, G-06/G-07, H-05/H-12. **Ни документ, ни зелёный document-CI не подтверждают реализацию**; нужен PostgreSQL + Axum + S3 + браузер на одном SHA.

**Асинхронный нюанс для F/G:** RIL контекст берётся и прикрепляется к typed outbox-событию **при исходном HTTP/PG commit**, с versioned паспортом либо проверенным provenance=server_committed. G-worker через несколько минут уже не видит исходного HTTP request; повторная классификация по маршруту фонового job недопустима. В старом signal_hygiene unknown/missing_context отклоняется, поэтому обязательный негативный тест: отложенное trusted ChapterCompleted проходит в F, а клиентский фальшивый trusted_completion без PG provenance не проходит. Подробнее [RIL договор](../contracts/ril-reader-signals-v1.md).
