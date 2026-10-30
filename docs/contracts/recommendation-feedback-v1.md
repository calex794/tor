# TOR — REC-13 v1: атрибуция рекомендаций и обратная связь

**Статус:** DRAFT, предложение линии F + интегратора I на 31.10.2026. Это технический контракт для review, не реализованный runtime, не применённая миграция и не подтверждённый uplift.
**Владельцы:** F — сбор/атрибуция, score-проекции и персональные решения; H — UI-видимость и команды; C — авторитетный прогресс/завершение; D — авторитетные покупки/возвраты; G — at-least-once доставка; A — consent/actor; B — видимость; I — общие схемы, миграции, HTTP/OpenAPI и wiring.
**Связь:** S044, F-07, REC-07/09/10/12/13/14, EVT-01..12. Источники: [recommendations execution](../roadmap/recommendations-execution-contract.md), [events/analytics](events-analytics-v1.md), [REC golden](recommendations-golden-v1.md), [release flags](release-flags-v1.json), [RIL signals](ril-reader-signals-v1.md).
**Исторический эталон:** pinned [NEwBOO attribution](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/services/attribution_service.py) и [quality signals](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/recommendations/services/attribution_quality_signal_service.py). Сохраняем сравнимые поля/события, а новую негативную политику версионируем отдельно.

## 1. Граница решения и инварианты

1. Рекомендация состоит из четырёх источников: behavioral/content/popularity плюс отдельный канал related_books. REC-13 не меняет математику recommendations_v1, tie-break, веса, max 50, quality boost max +50 или правило до 20 карточек в response.
2. Показ карточки, просмотр страницы, открытие книги, серверно подтверждённый прогресс, завершение главы и покупка — РАЗНЫЕ факты. HTML response, prefetch, S3 GET, paid grant, долгий открытый таб, scroll без PG progress и purchase не доказывают чтение.
3. Отсутствие события не становится самостоятельным событием «не нравится». Отрицательные показатели вычисляются с окном наблюдения и censored-классом; без достаточной экспозиции и телеметрии отрицательное состояние UNKNOWN.
4. Глобальный рейтинг не штрафуется за бездействие одного читателя. Персональное явное not_interested обрабатывается отдельно; автоматическое понижение за молчание выключено до самостоятельной экспериментальной версии политики.
5. Только сервер C может подтвердить ChapterStartedV1/ChapterCompletedV1, D — PurchaseCommittedV1; факт из клиента с полем trusted=true никогда не принимается. BusinessEventV1 и RoutePassportV1 определяются существующим events-analytics-v1, не дублируются.
6. F хранит только ограниченный по времени контекст атрибуции и агрегаты; никаких raw email, IP, текста главы, сырой истории путей, платных URL, DEK, чужого user_id и рекламных финансовых фактов.
7. B/A access policy и возрастные ограничения действуют при каждой окончательной выдаче, как в REC-10. Рекомендательная ссылка НЕ является entitlement. Hidden/blocked/deleted книги не должны ни показываться, ни раскрывать метаданные через атрибуцию.
8. Organic и оплаченные ads — разные продукты и счётчики. Organic qualified impression не является AdViewableConfirmedV1 и никогда не биллится.
9. При отказе аналитики посещение/чтение книги продолжается. Непроверенная телеметрия отклоняется; durable бизнес-события чтения/денег не теряются ради REC-13.
10. Пока нет согласия/правовой проверки, personalized_recommendations_enabled=false; только разрешённый публичный fallback. У аналитики и персональных предпочтений отдельное opt-in, не подразумеваемое самим RIL-согласием.

## 2. Жизненный цикл и классы доверия

- **Serve (F→H):** F выдаёт список canonical BookId после live CanDiscover. Для каждой разрешённой карточки H может получить от I/F короткоживущий подписанный ticket с assignment_id; сам факт выдачи ticket НЕ фиксируется как impression.
- **Visible (H→F):** браузер сообщает квалифицированный просмотр; это client claim и учитывается только в агрегированной воронке при валидном ticket/consent, не является trust-worthy подтверждением поведения.
- **Activate (H→F→B):** сервер проверяет actor/session, ticket, срок, законность показа и canonical BookId; фиксирует максимум один attributed click на assignment/book и безопасно переводит на обычный URL книги. Клики не дают права на платную главу.
- **Book open (F/H):** только успешный авторизованный book-detail resolve (200) может дать book_opened. Даже он не подтверждает чтение.
- **Reader (C→G→F):** C в собственном PG commit записывает валидированный ChapterStartedV1 или уникальный ChapterCompletedV1 (ReaderVersionManifest + точная версия/право); G доставляет event_id и сохранённую provenance, F идемпотентно связывает с подходящим click. Не восстанавливать provenance по маршруту worker.
- **Feedback (A/H→F):** аутентифицированный читатель с нужным согласием явно выбирает not_interested либо clear. Actor всегда из сессии, subject_token назначен сервером; поле чужого пользователя в body запрещено.
- **Batch (G/F):** из событий идемпотентно строятся daily агрегаты и диагностические воронки, затем — отдельные quality signals для нового поколения рекомендаций. Никогда не пересчитывать историю синхронно при GET.
- **Activation (REC-09/12):** новое поколение проходит quality/visibility/health gates, rollback готов. Экспериментальный feedback не может тихо изменять pinned v1 scoring.

| Сигнал | Источник | Класс | Допустимый вывод |
| --- | --- | --- | --- |
| recommendation_impression | H claim + проверенный ticket | client_observed | карточка, вероятно, видима; только аналитика |
| recommendation_click | F authenticated activate | server_verified_navigation | переход по recommendation ticket |
| recommendation_action/book_opened | H/F после canonical 200 | server_verified_navigation | книга открыта, чтение неизвестно |
| recommendation_action/chapter_started | C PG CAS + outbox | domain_committed | первый подтверждённый прогресс, не completion |
| recommendation_action/chapter_finished | C unique ChapterCompletedV1 | domain_committed | meaningful_read_v1 по действующему правилу |
| recommendation_action/library_added | C PG commit | domain_committed | сохранение книги, не чтение |
| recommendation_action/purchase_completed | D PG commit | domain_committed | покупка, не чтение |
| explicit not_interested или clear (отдельный preference command) | H/F session+CSRF | explicit_user_choice | только персональное решение, без обязательного assignment_id |

**Важно:** локальное отсутствие клика и отсутствие прогресса не добавляются в BusinessEventV1. Это вычисляемые состояния в агрегаторе с версией правил. Client signal никогда не получает domain_committed. Явный feedback хранится в rec_feedback_state и rec_feedback_commands без обязательного assignment_id: пользователь вправе выбрать not_interested на обычной странице книги.

## 3. Машинная схема и обязательный контекст

Файл [recommendation-feedback-v1.schema.json](recommendation-feedback-v1.schema.json) содержит JSON Schema 2020-12 для: ImpressionBatchRequest, ActivationCommand, ExplicitFeedbackCommand, InternalAttributionFact и DailyQualityAggregate.

Ticket — непрозрачное HMAC/AEAD-подписанное утверждение либо серверная ссылка на него; только доверенный сервер задаёт assignment_id (UUID), book_id (положительная canonical decimal string), source, reason, surface, position (1..20), generation_id, algorithm_version, session-bound subject/consent epoch, issued/expires и organic purpose. На клиенте не размещать subject_token, raw user ID, cookie hash или секрет подписи. Уважать rotation ключей, проверять scope/actor, аудит replays и clock skew. Ticket никогда не публикуется в URL, referrer, telemetry labels или URL книги. Для определения distinct visits сервер выпускает отдельный HMAC visit_scope_digest, не реальный ID сессии. Например, 30 минут простоя начинают новый visit; digest удаляется с сырыми событиями в пределах 30 дней.

**Пилотные технические пределы для review** (не доказанные метриками): ticket TTL 15 минут; квалифицированный organic view — >=500 промилле видимости карточки непрерывно >=1000 ms при document.visibilityState=visible; максимум 20 claims на запрос, одна засчитанная экспозиция на assignment/book. Эти пороги НЕ имеют отношения к рекламе и не могут использоваться для billable impression. Browser IntersectionObserver сообщает только claim; JS может быть подделан, иметь ошибки или отсутствовать.

Входные данные обязаны строго соответствовать schema, extra fields запрещены; неизвестные enum/algorithm_version отклоняются. Сервер назначает время приёма и доверенный контекст, не доверяет заявленному пользователем timestamp. В публичных метриках нет assignment_id, session/user ID или book_id как Prometheus label (book_id допустим только в защищённой bounded аналитической таблице).

## 4. Предлагаемый HTTP контракт (все новые routes заблокированы до I/OpenAPI)

Действующий OpenAPI GET /api/v1/books/{id}/recommendations описывает RecommendationPage/RecommendationItem и additionalProperties=false. Нельзя самовольно добавить ticket в ответ. I должен утвердить аддитивное optional поле в новой версии schema либо отдельный строго типизированный SSR sidecar; до этого доступен обычный безопасный GET без атрибуции.

| Метод и путь, предложение | Запрос и результат | Защита, ограничения |
| --- | --- | --- |
| GET /api/v1/books/{id}/recommendations | существующие items/generation_id/next_cursor, без изменения обязательных полей | текущая политика A/B; ticket только после I OpenAPI update |
| POST /api/v1/recommendations/impressions | ImpressionBatchRequest, 202 accepted, повтор безопасен | same-site Origin/CSRF, согласие, max 20, подтверждённый signed ticket, rate limit; не business truth |
| POST /api/v1/recommendations/activate | ActivationCommand, 303 See Other на canonical book URL | same-site form/CSRF, session binding, checked ticket, live CanDiscover, no arbitrary redirect; повтор ≠ второй click |
| POST /api/v1/me/recommendation-feedback | ExplicitFeedbackCommand, response {revision,choice} | только auth actor, consent, CSRF, expected_revision CAS и command_id dedup |
| GET /api/v1/me/recommendation-feedback | bounded собственные активные not_interested, cursor | auth actor+consent, никогда чужие subject rows |
| GET /api/v1/admin/recommendations/quality | обезличенный агрегат/health/holdout по дате/версии | только admin, bounded/censored, не публичные user-level факты |

Для API ошибок использовать существующий ErrorCode envelope: 400 syntax, 401 unauthenticated when required, 403 actor/consent/access, 404 hidden/unknown book without leaking existence, 409 stale revision, 422 invalid ticket/claim, 429 rate-limited, 503 unavailable. Даже при неработающей REC-13 клиент сохраняет обычную навигацию по канонической публичной ссылке (без attribution). Нельзя заставлять успешное чтение зависеть от аналитического POST.

Для доступности без JS форма активации может использовать стандартный POST + 303; прямой canonical link безопасен, но не атрибутируется. Серверный 303 — это факт навигации, но не доказательство внимания человека: bot/prefetch ограничиваются hygiene.

Пример request для client видимости:

~~~json
{
  "items": [{
    "event_id": "01f7ddec-3b73-46aa-8ad3-4eef9f97ce01",
    "ticket": "opaque-signed-ticket",
    "visible_fraction_per_mille": 650,
    "continuous_visible_ms": 1250,
    "document_visible": true
  }]
}
~~~

Пример явного исключения (только своё):

~~~json
{
  "book_id": "123",
  "choice": "not_interested",
  "expected_revision": 0,
  "command_id": "20a6bc9f-9e36-4b7b-ac10-6df35814096f"
}
~~~

## 5. PostgreSQL: модель, ключи и бюджеты (DDL-проект для I, НЕ миграция)

Репозиторий использует одну PostgreSQL и один outbox. Никаких новых message-broker/Volga/Spark на первом этапе. Линия F владеет только своими таблицами, I выпускает миграцию, а C/D остаются authoritative для чтения и денег.

~~~sql
-- Proposed, not runnable production migration.
CREATE TABLE rec_attribution_events (
  event_id uuid PRIMARY KEY,
  assignment_id uuid NOT NULL,
  subject_token bytea NOT NULL CHECK (octet_length(subject_token)=32),
  visit_scope_digest bytea NOT NULL CHECK (octet_length(visit_scope_digest)=32),
  book_id bigint NOT NULL CHECK (book_id > 0),
  event_type text NOT NULL CHECK (event_type IN
    ('recommendation_impression','recommendation_click','recommendation_action')),
  action_type text,
  source_event_id uuid UNIQUE,
  surface text NOT NULL CHECK (surface IN
    ('home','book_detail','reader_end','catalog','library')),
  source text NOT NULL CHECK (source IN
    ('behavioral','content','popularity','related_books')),
  reason text NOT NULL,
  position smallint NOT NULL CHECK (position BETWEEN 1 AND 20),
  algorithm_version text NOT NULL,
  generation_id text NOT NULL,
  trust_class text NOT NULL CHECK (trust_class IN
    ('client_observed','server_verified_navigation','domain_committed')),
  occurred_at timestamptz NOT NULL,
  received_at timestamptz NOT NULL DEFAULT now(),
  CHECK (
    (event_type='recommendation_impression'
     AND action_type IS NULL AND trust_class='client_observed')
    OR (event_type='recommendation_click'
        AND action_type IS NULL AND trust_class='server_verified_navigation')
    OR (event_type='recommendation_action'
        AND (
          (action_type='book_opened' AND trust_class='server_verified_navigation')
          OR (action_type IN ('chapter_started','chapter_finished',
                             'library_added','bookmark_added','purchase_completed')
              AND trust_class='domain_committed' AND source_event_id IS NOT NULL)
        ))
  )
);
CREATE UNIQUE INDEX rec_one_impression_per_card
  ON rec_attribution_events(assignment_id,book_id)
  WHERE event_type='recommendation_impression';
CREATE UNIQUE INDEX rec_one_click_per_card
  ON rec_attribution_events(assignment_id,book_id)
  WHERE event_type='recommendation_click';
CREATE INDEX rec_attribution_subject_book_time
  ON rec_attribution_events(subject_token,book_id,occurred_at DESC);
CREATE INDEX rec_attribution_retention
  ON rec_attribution_events(received_at);

CREATE TABLE rec_quality_daily (
  bucket_date date NOT NULL,
  book_id bigint NOT NULL CHECK (book_id > 0),
  surface text NOT NULL,
  source text NOT NULL,
  reason text NOT NULL,
  position_bucket smallint NOT NULL CHECK (position_bucket BETWEEN 1 AND 20),
  algorithm_version text NOT NULL,
  generation_id text NOT NULL,
  qualified_impressions bigint NOT NULL DEFAULT 0 CHECK (qualified_impressions>=0),
  paired_clicks bigint NOT NULL DEFAULT 0 CHECK (paired_clicks>=0),
  unpaired_clicks bigint NOT NULL DEFAULT 0 CHECK (unpaired_clicks>=0),
  book_opens bigint NOT NULL DEFAULT 0 CHECK (book_opens>=0),
  confirmed_starts bigint NOT NULL DEFAULT 0 CHECK (confirmed_starts>=0),
  confirmed_completions bigint NOT NULL DEFAULT 0 CHECK (confirmed_completions>=0),
  censored_count bigint NOT NULL DEFAULT 0 CHECK (censored_count>=0),
  calculation_version smallint NOT NULL DEFAULT 1,
  CHECK (paired_clicks<=qualified_impressions),
  PRIMARY KEY (bucket_date,book_id,surface,source,reason,position_bucket,
               algorithm_version,generation_id)
);

-- Separate preference, never inserted as an automatic no-click penalty.
CREATE TABLE rec_feedback_state (
  subject_token bytea NOT NULL CHECK (octet_length(subject_token)=32),
  book_id bigint NOT NULL CHECK (book_id>0),
  choice text NOT NULL CHECK (choice IN ('not_interested','clear')),
  revision bigint NOT NULL CHECK (revision>=1),
  last_command_id uuid NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now(),
  expires_at timestamptz NOT NULL,
  PRIMARY KEY (subject_token,book_id)
);
-- Dedup/cached result per personal command. Commit atomically with CAS.
CREATE TABLE rec_feedback_commands (
  subject_token bytea NOT NULL CHECK (octet_length(subject_token)=32),
  command_id uuid NOT NULL,
  book_id bigint NOT NULL CHECK (book_id>0),
  resulting_revision bigint NOT NULL CHECK (resulting_revision>=1),
  recorded_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (subject_token,command_id)
);
CREATE INDEX rec_feedback_commands_expiry
  ON rec_feedback_commands(recorded_at);
~~~

**DDL детали для миграции I:** после review добавить ограничения длины reason/algorithm/generation, индексы по указанным PG workload, приватные roles/GRANT, partition/TTL без удаления чужих canonical facts, и запрет некорректной пары event_type/trust_class. Интегратор проверяет DDL на реальном disposable tor_ci PG18, права tor_app и план запроса на 100k книг.

Для explicit feedback применить CAS по (subject_token,book_id) и expected_revision с atomic insert receipt (subject_token,command_id). Повтор в пределах 30 дней возвращает исходный revision, даже если более новая команда уже изменила состояние. После удаления receipts принимаются только свежие команды с действительным короткоживущим session-bound CSRF proof. Clear сохраняет краткоживущий tombstone для CAS, не вечную историю.

Идемпотентное построение daily-счётчиков: occurred_at в UTC отвечает за порядок действий, received_at — за задержку доставки/ретенцию. Каждое исходное событие участвует максимум один раз (event_id receipt или пересборка bounded window из фактов), counts и receipt фиксируются одной транзакцией. At-least-once из G не удваивает значения; запоздалое событие корректно пересчитывает открытый дневной bucket, но не меняет закрытый отчёт без контролируемого versioned backfill. Raw events удалить не позднее 30 дней, агрегаты не позднее 180 дней; персональное состояние по умолчанию TTL 30 дней либо удаляется при clear/opt-out. Финансовые C/D факты и их retention не меняются.

## 6. Детеминированная атрибуция и окна

**Предложение стартовой политики для эксперимента; не считать доказанной оптимальной конфигурацией:**

1. Для пары (subject_token, book_id) выбрать **последний валидный подтверждённый click**, предшествующий действию; tie: occurred_at DESC, assignment_id ASC. Ранее созданное чтение не становится новой конверсией. Действие без подходящего click учитывается в своей предметной аналитике, но НЕ как результат рекомендации.
2. Attribution window после click: ChapterStartedV1 не позже 7 суток, первый новый ChapterCompletedV1 не позже 14 суток. Событие C обязано иметь свой оригинальный server committed occurred_at и event_id; received_at отражает только время доставки; если после возвращения через 10 дней читатель завершил главу, это может быть post-click completion, но не «start within 7d».
3. Клик без видимого impression (JS отключён/заблокирован) — **unpaired click**: он не улучшает CTR, но навигация разрешена. Qualified view сам по себе не создаёт training event.
4. Через 72 часа после подтверждённого book_open можно вычислять provisional no_verified_start. Если C не получил progress, есть ошибка, неразрешённая глава, paywall, hold, opt-out, задержка outbox или срок ещё не истёк, значение UNKNOWN/CENSORED, не отрицание интереса. При позднем достоверном progress метрику пересчитать.
5. Для no_click необходимы >=3 отдельных квалифицированных показов в разных сессиях в течение 7 суток. Это **только shadow diagnostic**, никогда автоматическое отрицательное событие или штраф v1. Отсутствие view telemetry не трактовать как отсутствие клика.
6. Для A/B holdout и измерения causality использовать стабильное назначение субъекта по keyed hash(subject, experiment, policy_version), но не засчитывать A/B как честный uplift без фактической выборки, sample ratio и доверенных читательских событий.

При двух одновременных устройствах credit выдаётся максимум одному assignment. Счётчик «первое подтверждённое завершение после рекомендации» дедуплируется по (source_event_id) и группе (subject_token,book_id,policy_version) на выбранном окне; повторный completion или другое устройство не создают второй first conversion. Несколько глав в книге можно отдельно агрегировать как continued_read, но не выдавать за несколько новых привлечённых читателей.

В прошлой Python-модели качество строилось из CTR/action/purchase ratios с sample confidence и anti-fraud trust buckets; REC-13 сохраняет совместимость с ними в дифференциальных тестах, но НЕ вводит другой вес в уже pinned recommendations_v1. Смена бонуса, глобальные минусы и автоматический re-rank — отдельный одобренный policy_version/v2.

## 7. Новая негативная политика (в v1 безопасно выключена)

- **Explicit not_interested:** только аутентифицированный reader+consent, POST с CSRF и expected_revision; скрыть из личных organic surfaces, не из публичного каталога. Выбор clear восстанавливает возможность показа. Нельзя менять чужой профиль.
- **Repeated no-click:** v1 лишь агрегат наблюдения; будущая отдельная политика может временно снижать частоту показов конкретному читателю, только после review и A/B. Не удалять книгу глобально, не обучать без consent.
- **Open without start:** v1 диагностическая конверсия после зрелого окна, не персональный hard negative. Платный барьер, проблемы доставки, hidden/18+/deleted, tracker-loss и недостаточная проверка прогресса приводят к CENSORED.
- **Unknown:** нейтральное состояние. Не додумывать просмотр и не штрафовать за клиентские блокировщики или отказ от трекинга.
- **New books:** сохранять exploration/fair exposure и cold-start; небольшой объём показов не должен превращать книгу в постоянно низкий рейтинг.

Явное личное исключение не является авторитетом B/A и не влияет на чужие рекомендации; пользователь может отменить его. Opt-out немедленно блокирует новые персональные записи и удаляет производные feedback/attribution у F, сохраняя независимые законно необходимые C/D факты.

## 8. Метрики и защита от подмены

- qualified CTR = paired_clicks / qualified_impressions для одной сравнимой когорты; без деления при 0 denominator. Отдельно считать unpaired_clicks.
- click→start = distinct attributed first ChapterStarted / eligible attributed clicks (после полного 7-дневного окна); click→completion = distinct first completed book / eligible attributed clicks (14-дневное окно). Из-за разных окон отчёты когортизируются по времени click, а не произвольной календарной дате событий.
- meaningful_read_v1 = оригинальный C ChapterCompletedV1. «Покинул книгу»/«не понравилась» не являются доказанными событиями.
- Дополнительные safety-метрики: unmatched events, client-only impressions, missing consent, callback errors, stage drop-off by position, new-book exposure, fallback rate, visibility leaks (требуется 0), late event correction, outbox lag, rollout hold/rollback, P95 query time.
- Метрики не экспортируют high-cardinality book_id/subject_token/assignment_id в Prometheus labels; per-book stats только в ограниченном защищённом read-model для оператора.
- Нулевой CTR не означает «плохую книгу». Позиция, обложка, сезонность, язык, доступность текста, холодная выборка и способ сбора просмотров создают смещение. Для автоматических решений нужен контроль с holdout и доверительным интервалом/достаточной мощностью, а не порог «3 показа» без данных.
- Клиентские попытки подделать qualified view, replay ticket, произвольный UserId, BookId, unauthorized route/provenance и click-farming не могут создавать domain_committed. В подозрительном случае понизить trust или отвергнуть сигнал, но не менять canonical покупку или C completion.

## 9. Управление релизом и отказами

- Требуются отдельные feature flags (пока **предлагаемые**, NOT in release-flags-v1.json): rec_attribution_collection_enabled, rec_feedback_explicit_enabled, rec_feedback_inferred_shadow_enabled. Автоматическое применение inferred negatives/quality policy только новой явной версией после эксперимента; default OFF.
- Существующий personalized_recommendations_enabled остаётся OFF до REC-M01..12, EVT-01..12, REC-09/10/12/13/14 и consent/access/Chromium/PG acceptance. Public fallback продолжает работать.
- PG/G аналитика недоступна → навигация не ломается, attribution пропускается; никаких старых приватных кешей. Храним диагностический aggregate отказов без raw identifiers.
- Rollout 1→5→10→25→50→100% только после quality/health gates, возможность hold/rollback. Нельзя заявлять повышение CTR/meaningful_read по зеленому document CI или синтетической выборке.
- No Volga/stream processor dependency: текущий G и PG достаточно до замеров. При нагрузке расширяем bounded batch materialization, а не переносим domain truth в новый движок.

## 10. Обязательные приёмочные сценарии REC-FB-01..20

| ID | Испытание | Ожидание |
| --- | --- | --- |
| REC-FB-01 | GET карточек, JS закрыт/выключен | 0 qualified impressions, чтение доступно |
| REC-FB-02 | скрытая/невидимая карточка, предзагрузка | 0 qualified impressions |
| REC-FB-03 | видимая 499‰/999ms vs >=500‰/1000ms | только квалифицированная claim принимается как client_observed |
| REC-FB-04 | тот же assignment/book повторно 10 раз/2 tabs | ровно 1 impression и не больше 1 click |
| REC-FB-05 | подпись ticket изменена/истёк TTL/другая сессия | нет attribution и утечки; forged отвергается, expired authentic может перейти только при B/A доступе |
| REC-FB-06 | fake client ChapterCompleted, trusted=true | 0 domain_committed, нет quality boost |
| REC-FB-07 | C valid first progress + last manifest block | 1 start и 1 completion, source event_id уникален |
| REC-FB-08 | paid grant/S3 GET/purchase без progress | 0 meaningful_read |
| REC-FB-09 | duplicate delivery, restart G, out-of-order event | daily counters и first conversion не удваиваются |
| REC-FB-10 | click без prior JS impression | unpaired_click, CTR не повышается |
| REC-FB-11 | book_opened, нет начала, окно ещё не истекло | pending, не отрицание |
| REC-FB-12 | book_opened, C outage/paywall/hold/deletion | censored, нет штрафа |
| REC-FB-13 | поздний C progress после provisional no-start | event recomputation снимает misleading no-start |
| REC-FB-14 | 3 видимых показа без клика в разных sessions | shadow metric, нет live penalty |
| REC-FB-15 | чужой actor/CSRF/consent OFF/opt-out | denied, no personal storage |
| REC-FB-16 | свой not_interested, repeat, clear, stale revision | идемпотентно и CAS; только свой feed |
| REC-FB-17 | 2 устройства и 2 clicks до 1 ChapterCompleted | deterministic last eligible, ровно 1 conversion |
| REC-FB-18 | raw 30d/aggregate 180d, erasure/consent epoch rotation | приватное удалено, независимые C/D факты не затронуты |
| REC-FB-19 | public/paid/18+/draft/hidden vs old generation | CanDiscover live, ни метаданных, ни paid access не утекло |
| REC-FB-20 | 100k books + PG fail + browser + holdout | bounded query/latency, safe fallback, никакого фиктивного uplift |

**Гейты доказательства:** pure schema/contract tests → реальные PostgreSQL18 и 2 actors → Axum HTTP+CSRF+RIL/outbox restart → Chromium видимость/клики/переходы/S3 → anti-fraud/retention/holdout/100k query plan → последняя проверка точного commit SHA. Документы/PR сами по себе ни одного из этих тестов не подтверждают.

## 11. Инкременты, владельцы, блокировки

1. **I+F contract review:** утвердить JSON Schema, typed ports/Actor, OpenAPI extension, error codes/flags, migration plan. До этого нельзя объявлять HTTP маршруты действующими.
2. **F+H capture:** signed assignment, protected visible_impression/activate, server-side book_open, explicit feedback self only, browser tests.
3. **C+G+F business attribution:** C committed progress/completion, immutable RIL provenance in outbox, dedup receipts and last eligible click; actual PG restart tests.
4. **F aggregates/quality:** bounded PG materialization, daily metrics, censorship, anti-fraud, opt-out/TTL; legacy NEwBOO quality comparison. No score change yet.
5. **REC-12 experimental policy:** holdout, per-user explicit suppressions, shadow negatives, eventual v2 separately reviewed. Do not merge all five in one feature PR.

**Приёмка REC-13:** реальный click → проверенный C progress/completion → ровно одна корректная attributable conversion и защищённая aggregate запись; фальшивый client completion/paid grant/скрытая книга не обучают и не раскрывают данные; consent/erasure/rollback работают на PG+browser. Никакой вывод о business uplift без реальной контрольной группы.

**Статус на 31.10:** Draft PR #309 — чистый hybrid scoring; Draft PR #320 — live B public metadata assembly, stacked на #309. Оба — не main, не REC-13. Этот документ только уточняет работу для I/F/G/H/C, а не выполняет её.

