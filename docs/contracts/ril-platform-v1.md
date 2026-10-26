# TOR — RIL: общесистемная архитектура маршрутов и поведенческой наблюдаемости

**Статус: восстановленный общий контракт TOR, не работающая реализация Rust.** RIL — самостоятельная общесистемная инфраструктура NEwBOO, **не** часть S3, рекомендаций или читалки. Договор переносится поверх действующих S3/FIN/ACL/REC/EVT контрактов и необходим исполнителям BookaRust/TOR до параллельной разработки. Он уточняет существующие 117 задач и не образует десятую линию.

**Исходные доказательства NEwBOO:** [Route Intelligence](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/route_intelligence_service.py), [middleware и tracing](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/main.py#L1584-L1649), [Behavior Sketch](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/behavior_sketch_service.py), [Behavior Events](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/behavior_event_contract_service.py), [Behavior Paths](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/behavior_path_offline_worker_service.py), [Route Governance](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/route_governance_service.py), [route fingerprint/snapshot](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/route_governance_snapshot_service.py), [cross-domain acceptance](https://github.com/HeisLuka/newboo/blob/17125ef/backend/tests/cross_domain/test_ril_acceptance.py).

## 1. Три отдельных слоя

**RIL Core:** классифицирует зарегистрированные HTTP-маршруты по известности, семейству, поверхности, предметной области, типу операции/стоимости и риску. Вычисляет единый bounded RoutePassport, в котором нет URL с пользовательскими ID. Не выполняет purchase/ACL, не принимает самостоятельных решений о блокировках, не строит рекомендательный рейтинг.

**Behavior Intelligence поверх RIL:** нормализует технические request-events, строит ограниченные субъектные sketches, переходы между route codes, интервалы, петли, нарушения и объяснимые risk buckets. Имеет hot bounded memory deltas + durable hourly/daily aggregates и offline path analysis. Это отдельная стадия, а не расширение классификатора неограниченным состоянием.

**Route Governance поверх RIL и наблюдаемости:** сверяет реально зарегистрированные маршруты с taxonomy, политиками auth/CSRF/rate-limit/ownership, внешним nginx exposure, SEO routing parity и измеренными затратами. Формирует coverage, findings, routing fingerprint, rollout safety, карту самых дорогих маршрутов и админскую диагностику.

**Потребители общего контекста:** A security/antibot/action limiter, I tracing/load shedding/CI/FinOps, F SEO/recommendations/advertising, G durable metrics, audit, H admin interface. Каждый потребитель применяет СОБСТВЕННУЮ политику. RIL известный маршрут не означает наличие ACL-права.

## 2. RoutePassportV1 — точные поля

Поля из Python: `route_known` bool; `route_family` (bounded static family); `route_match_kind` exact/prefix/fallback/unknown; `route_decision` allow/diagnostics/allowlisted/deny; `traffic_surface` public_page/api/internal/admin/static/service/unknown; `product_area` reading/catalog/auth/billing/admin/advertising/dashboard/social/mail/notifications/recommendations/marketplace/storage/system/unknown; `cost_class` free/cheap_read/expensive_read/write/background/unknown; `risk_hint` normal/suspicious/scanner/unknown; версии `ril_taxonomy_v1` / `ril_classifier_v1` / `route_known_v1` либо специально согласованный TOR version. Unknown route не маркировать как normal.

**Axum/I:** строить immutable registry routes + taxonomy/method overrides **один раз**, классифицировать по framework matched route template и HTTP method, не делать linear scan каждого запроса. Request context и trace содержат единственный паспорт. Не подставлять raw /book-slug, user_id, chapter_id, IP, UA, request_id или presigned query в Prometheus labels; bounded cardinality. Без данных RIL security authorization обязана продолжать безопасно работать.

**Граница:** RIL возвращает признаки. UnknownRoutePolicy, anti-bot enforcement, rate limit, SEO policy, доступ к платной главе, оплата, рейтинг и рекламный billing находятся вне core. Старый NEwBOO явно проверял этот раздел — см. [dependency boundary tests](https://github.com/HeisLuka/newboo/blob/17125ef/backend/tests/architecture/test_seo_ril_dependency_boundary.py) и [unknown-route policy](https://github.com/HeisLuka/newboo/blob/17125ef/backend/app/services/unknown_route_policy_service.py).

## 3. Behavior Sketch / Paths / retention — G/I

**В исходниках:** per-subject bounded LRU на authenticated/guest/anonymous; HMAC pseudonymous subject hash; counts route/transition/status/db calls/errors/latency, denied/unknown/scanner, time gaps instant/fast/normal/long; versioned score normal/watch/suspicious/scanner_like/abusive; объяснимые contributions; bounded hourly route and transition deltas; отдельный offline worker извлекает path/frequency/friction и hygiene normal/watch/degraded/reject.

**PG проект на смену YDB:**
- `ril_route_hourly`: time_bucket, bounded subject_class/route_code/route_family/known/decision/surface/area/cost/risk/schema, агрегаты HTTP status classes, duration buckets, db_calls/errors/total_ms, slow_count.
- `ril_transition_hourly`: time_bucket, subject_class, from_route_code, to_route_code, gap_bucket, count (ограничение cardinality).
- `ril_daily_rollup`: периодическое свертывание и retention, не создавать строку для каждого произвольного URL.
- `behavior_paths`: версионированные compact path tokens/features/hygiene, без бесконечного персонального хранения и с утверждёнными TTL/opt-out.

**Hot path:** request finalizer строит bounded compact event и обновляет ограниченный in-memory sketch/буфер. G batch-flush в PG с retry/lease/fence, не делать синхронный PG INSERT для каждого HTTP. При перегрузке сбрасывать только НЕкритичную телеметрию с явным drop counter; committed purchases/progress/entitlement — отдельный durable outbox и никогда не теряются из-за перегрузки metrics.

**Offline worker:** сортирует события в session, формирует route sequence и паузы, фиксирует loops/backtrack/unknown/scanner-like paths. Риск/гигиена — диагностические признаки; конкретные меры A/F реализуются независимо, с учётом ложных срабатываний. Длинная пауза читателя сама по себе не признак атаки.

## 4. Route Governance и карта затрат — I

1. На каждом изменении реестра сравнить фактический routing graph + HTTP method с taxonomy. Missing taxonomy для известной route family и known route with unknown taxonomy — blocking findings; stale taxonomy — предупреждение.
2. Сверить отдельные RoutePolicySpec auth/role/owner-or-member/CSRF/action-rate-limit, Nginx public/internal exposure. Несанкционированный public admin endpoint и отсутствующая защита write-route — security blocker.
3. Проверить routing fingerprint и SEO route parity; registrar list, observations/SEO должны понимать одну route family, но SEO rendering не импортирует RIL internals.
4. Из trace и bounded long-window deltas построить Route Cost Map: calls, error count/rate, duration, DB calls/errors/time, estimated compute/request price, top costly/slow/error/db routes и trends. В отличие от простого Prometheus это actionable prioritized improvement list.
5. Автоматическое агрессивное unknown blocking/load shedding **только после governance safety gate**: critical gaps → blocked, некритичные mismatch/unknown observed → diagnostics-only, clear → safe.
6. Приложить полный read-only admin report с SHA/fingerprint/coverage/cost/security findings. Не раскрывать user IDs и сырые подписанные URL.

## 5. Контракты потребителей

| Область | Какие данные потребляет | Чего RIL не решает |
|---|---|---|
| A security | unknown/scanner/behavior deviation, rate scope | JWT/rights, bans, CSRF policy |
| I Observability+FinOps | route passport, response, latency, DB counters | product/business policy |
| F SEO | matched route, taxonomy/parity and runtime diagnostics | canonical/robots/sitemap |
| F recommendations | RIL/path hygiene + separate trusted business signals | ranking, personalization |
| F advertising | suspicious/scanner context + verified impressions | financial billing |
| G analytics | typed compact telemetry/deltas/events | release/enforcement policy |
| H admin | read-only governance/traffic/cost report | taxonomy edits in UI |
| D finance | cost analytics context for business operation | wallet and entitlements |

Важно: подписанный GET S3 — не Axum route и не источник определения RIL. Он может иметь отдельный storage accounting, **но S3 не определяет назначение RIL**. [Читательская интеграция RIL](ril-reader-signals-v1.md) — узкий потребитель этого общего механизма.

## 6. Точные владельцы и исполняемые этапы в существующих 117 задачах

**I** владеет `crates/route_intelligence` (целевой pure crate: taxonomy, classifier, passport, projection, route governance, cost map; без Axum/SQLx), `crates/contracts` и Axum middleware+CI wiring. **G** владеет `crates/traffic_behavior` (целевой bounded sketch/path/PG projections/jobs), данные SQL только в repository. **A** применяет security policy; **F** SEO/ads/rec consumers; **H** admin read-only. Root Cargo, module registry и shared migrations меняет только I.

| Подшаг | Внутри задачи | Конкретный результат | Минимальная проверка |
|---|---|---|---|
| RIL-01 | I-02 | Зафиксировать эталон taxonomy/method overrides/версии из NEwBOO | Golden known/unknown/fragment/login |
| RIL-02 | I-02 | Pure RoutePassport + match классификатор + static registry | Нет HTTP/SQL, bounded labels |
| RIL-03 | I-13 | Axum instrumentation once, status/latency/DB summary, request context | Нет per-request route graph scan |
| RIL-04 | A-09 | UnknownRoutePolicy, scanner/rate/load policies отдельно | Known не обходится; unknown safe |
| RIL-05 | G-06 | Bounded per-subject sketch + local deltas | 10k subjects, bounded RAM |
| RIL-06 | G-07 | PG hourly/daily rollup + durable jobs/retention | Restart/drop/backfill проверены |
| RIL-07 | G-10 | Offline paths/loops/friction/hygiene | Normal/watch/degraded/reject |
| RIL-08 | I-07 | Coverage + ACL/role/CSRF/rate governance CI | Новая unclassified route fail |
| RIL-09 | I-07 | Nginx exposure & routing fingerprint | Private admin exposure fail |
| RIL-10 | I-09 | FinOps route costs/top latency/db/error impact | Long-window rankings |
| RIL-11 | F-08 | SEO/RIL parity without producer import | Mismatch detected |
| RIL-12 | F-07/F-10 | Rec/ad hygiene separate from billing | Scanner not billed/trained |
| RIL-13 | H-07 | Read-only admin RIL dashboard | Admin only, no PII |
| RIL-14 | I-12 | PG/Axum/nginx/browser acceptance + safety rollout | SHA/CI and runtime proof |

**Последовательность:** I-02/13 сначала формирует typed passport/registry, независимо A-09/политики; после freeze G sketch/rollup, I governance, затем F/H consumers. Изменение общих типов запускает нужный affected-test set, изменение F weights не запускает все чистые route unit tests, но обе стороны проходят contract tests. Не вводить 118-ю задачу.

## 7. Негативные и качественные тесты

T1: 100k разных book ID → фиксированный bounded route_family, неизменное число metrics series. T2: 500–1000 routes, измеренный p95 classifier, no linear scan. T3: missing taxonomy/CSRF/owner guard → CI finding; fail closed при опасном состоянии. T4: unknown scanner vs legit nested route и method override (GET/POST login) — никаких ложных ошибок. T5: настоящее Nginx/Ingress exposure не публикует приватный API. T6: Bounded 10k subjects/routes/transition overflow не выходит за memory budget. T7: DB outage telemetry does not crash auth/read/write; ledger and outbox still durable. T8: Route Cost Map order by real DB cost/duration/errors; no PII. T9: different domain consumers receive same taxonomy, but independent enforcement decisions. T10: browser + API+PG verify legitimate user flow and no leak. T11: admin report shows versions/coverage/deltas/fingerprint and no sensitive labels. T12: load-shedding enforcement cannot be enabled on unsafe governance snapshot.

## 8. Открытые решения до production

Согласовать privacy/retention/opt-out pseudonymous behavior; точные memory/cardinality/max event limits на домашнем 16-ГБ сервере после нагрузочного теста; Nginx vs другой reverse proxy для external exposure audit; безопасный rollout enforcement diagnostics→audit→enforce. Это **параметры инфраструктуры и продукта**, не попытка переписать семантику RIL.

Текущий GitHub/Tela содержал слишком узкое описание RIL через S3/reader events. Этот документ становится **главной спецификацией RIL**; [reader-signals](ril-reader-signals-v1.md) — частный consumer.