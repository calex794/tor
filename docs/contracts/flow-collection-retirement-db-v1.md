# FLOW / REC v2 P2b1 — барьер записи и ограниченное удаление

## Граница решения

Основание и пользовательский путь заданы в
[flow-rec-runtime-retirement-v2](flow-rec-runtime-retirement-v2.md).
P2b1 закрывает прежнее индивидуальное накопление в PostgreSQL и удаляет его
производные. Он не создаёт новый collector, новый EventStore или модель
рекомендаций. P2a catalog fallback и явные P1-скрытия продолжают работать.

Этот файл задаёт проверяемые требования. Приёмка требует exact SHA и фактических
PG/HTTP/browser проверок на нём. Ни ранее испытанный SQL черновик, ни один markdown
не подтверждают новые байты. Production операции этим изменением не разрешаются.

## Миграции и сохранённые данные

| Граница | Назначение |
| --- | --- |
| 190 / 202611158256 | Принятые независимые явные предпочтения P1 |
| 191 / 202611163274 | Только таблица invalidation и функции, без attached barriers |
| 192 / 202611168903 | Атомарные fences, empty C ports, DELETE hooks и batch API |

Старые 190 файлов и SQLx checksum не изменяются. 191 нужна изолированным
историческим тестам текущего read-only report SQL; она не считается отдельной
production приёмкой. Штатный runner применяет весь набор, повторно — без действий.

Сохраняются без изменения P1 choices, feedback epoch, revisions, command keys,
payload hash, результаты, timestamps и исходный TTL. Общий явный namespace
consent/feedback receipts не превращается в поведенческий профиль.

Canonical C starts/completions, позиции, библиотека, сохранённые редакции и follows,
purchase/version evidence и общий outbox payload/ID/dedupe не удаляются.
Эти бизнес-источники содержат user IDs и не называются анонимными.

## Прекращённые writers

Statement BEFORE INSERT OR UPDATE fences закрывают даже zero-row старые запросы.
SQLSTATE 55000 означает retired collection. Они не зависят от app GRANT,
старого enabled scope, GUC или operator flag.

| Группа | Stores |
| --- | --- |
| Согласия/профиль | ril_behavior_consents, rec_subjects |
| REC | rec_assignments, rec_attribution_events, rec_first_conversions, rec_reader_signals, rec_dirty_buckets, rec_quality_daily, rec_worker_health |
| Organic | organic_discovery_touches, organic_discovery_first_conversions, organic_discovery_unattributed_reads, organic_discovery_dirty, organic_discovery_quality_daily |
| FLOW visits | flow_visits, flow_visit_receipts, flow_visit_account_days, flow_visit_continuations |
| FLOW books | flow_book_cohorts, flow_book_return_anchors, flow_book_action_receipts, flow_book_worker_health |
| Optional C | reader_activity_days, reader_return_subjects, reader_book_return_subjects, reader_flow_book_actions, reader_flow_activity_days, reader_flow_read_receipts |
| Старые returns | reader_return_daily, reader_book_return_daily, reader_return_coverage, reader_book_return_coverage |

FLOW visit/book daily не могут расти: INSERT запрещён, UPDATE допускает только
те же ключи и невозрастающие счётчики; прежние nonnegative/funnel CHECK остаются.
История без retained individual source не удаляется только ради формального нуля.
Coverage закрывается по последнему существующему heartbeat, не по выдуманному
моменту непрерывного сбора.

Approved reader_activity_consent и reader_flow_consent сохраняют типы и простой
row-lockable SELECT с false. C verified action/day/read-receipt ports пусты;
historical canonical reader_flow_verified_read_events сохраняется.
AFTER INSERT capture callbacks на library/follow/start/completion — no-op.
Старый C activity INSERT SELECT может получить 55000 в своём optional savepoint:
бизнес commit сохраняется, старый numeric capture-failure допустим.
Новый runtime P2a вообще не вызывает optional capture.

## Доказанное FLOW-вычитание

AFTER DELETE STATEMENT transition tables содержат ровно удалённые raw rows.
По отсортированным daily keys вычитается их finalized contribution.
Scope helpers вызывают DELETE, не повторное арифметическое вычитание.
Одинаковый helper, scope hook, старый TTL DELETE и FK cascade поэтому не могут
дважды снять один retained вклад.

Отсутствующий ещё сохраняемый FLOW daily вызывает 23514 и полный rollback.
Счётчики не clamp-ятся. За старым FLOW cutoff today UTC minus 179 days
ретракция уже истёкшего bucket не требуется. Whole-book canonical DELETE может
удалить соответствующий book daily по FK; тогда его отсутствие допустимо.
Удаление аккаунта сохраняет анонимный остаток других наблюдений.

Уникальность аккаунтов требует сохранённых account-day rows; число страниц
продолжений требует actual receipt за тот же период. Суточный
агрегат не подтверждает наличие этих исходников. После их удаления показатели
равны None даже при положительном сохранённом flow_visit_daily; они не становятся
нулями. Числовые архивные visits/pageviews остаются самостоятельным измерением.

Порядок для FLOW — book daily, затем visit daily; ключи внутри каждой таблицы
сортируются. Scope erasure вызывается до поворота/удаления scope.
После барьера INSERT/UPDATE scope запрещены, DELETE сохраняется.

## REC / organic: отсутствие доказательства вместо ложного числа

Порт отсутствия данных:

    rec_analytics_invalidations(source text, bucket_date date, book_id bigint)
    PRIMARY KEY(source,bucket_date,book_id)

source имеет четыре значения: recommendations, catalog, search, unattributed.
В таблице нет subject, account, event ID или raw URL. FK на books отсутствует:
hard deletion книги не может восстановить полноту глобального source отчёта.

Прежний materializer не хранит exact raw-to-daily marker. Поэтому удаление
assignment/touch/unattributed row атомарно ставит marker его source/day/book;
REC dirty key ставит recommendations marker, organic dirty key — три organic
канала того же day/book. Последний не содержит source, и угадывать его нельзя.

Marker делает неизвестным соответствующее измерение до удаления daily:
report SQL маскирует его прежде totals, group aggregation, pagination,
classification, rates и current/previous deltas. Частичный SUM не изображает
полный измеренный итог. UnknownEvidence отличается от NoObservations.

Recommendations затрагивает impressions/clicks/book_opens/starts;
catalog/search — issued/clicks/book_opens/starts; unattributed — starts.
Source report показывает неизвестность своего затронутого периода, включая
completions; у unattributed навигационные поля по-прежнему не измеряются.
Books/groups сохраняют принятую текущую eligible population и маскируют
только соответствующие измерения. Global source не фильтрует старые ключи
по сегодняшнему каталогу. Незатронутые книги/каналы/дни сохраняются.

Физическое удаление marked daily выполняется отдельными ограниченными порциями.
Marker сохраняется до завершения этой работы и до конца прежнего 180-day окна:
cutoff строго bucket_date < today UTC minus 180 days. На самом граничном дне
marker остаётся. Даже за cutoff marker не удаляется, пока существуют совпадающие
daily/raw/dirty rows. Окна воскресших цифр между delete и marker быть не должно.

## Batch, конкуренция и время

Единственный узкий F→I SQL порт:

    rec_retire_analytics_batch(p_limit integer DEFAULT 256)
      RETURNS TABLE(erased_rows bigint, pending boolean)

Допустим 1..1000; NULL/вне диапазона — 22023. Runtime передаёт 256.
Одна транзакция ограничивает сумму DELETE, normalized outbox и scrubbed RIL
work rows значением limit. erased_rows считает только фактические DELETE.
Дополнительные изменения ограничены: до одной FLOW daily retraction на удалённый
raw input, до 3*limit marker inserts. Это не обещание limit всех SQL row updates.
FK leaves идут раньше parents; parent NOT EXISTS покрывает все actual children.
Положительный FK fan-out не скрывается за returned DELETE count.

Transaction advisory key (20261116,8903) не хранит личную историю.
Batch сначала делает pg_try_advisory_xact_lock; busy возвращает ровно (0,true).
Scope helpers берут тот же gate в начале. BEFORE DELETE STATEMENT hooks на
retired tables и users/books/outbox берут его до row locks и каскадов.
Обычные canonical INSERT/UPDATE, P1 команды и чужие consumer ACK этим gate
не сериализуются. Он не является авторизацией и не даёт elevated privileges.
Все новые SQL functions и retained erasure hooks имеют
search_path=pg_catalog,public,pg_temp; public proof нельзя подменить temp table.

F adapter открывает короткую транзакцию, ставит lock_timeout=250ms и
statement_timeout=750ms до SELECT, проверяет returned count и commit.
Сам SQL port имеет lock_timeout=250ms. Таймаут/ошибка откатывает всю порцию.
SKIP LOCKED сохраняется; занятые rows пропускаются. Advisory busy не ошибка.
Старый caller, заранее удержавший произвольный row lock, всё ещё может создать
цикл: обязательны timeout, rollback, retry и остановка старого процесса.

pending учитывает raw, собственные deliveries, dirty keys, marked/expired daily,
старые outbox obligations, RIL staging и просроченные removable markers.
Он может консервативно остаться true до следующего прохода. erased_rows=0
сам по себе ничего не доказывает. pending=false не доказывает состояние RAM.

Always-on supervisor запускается при AUTH=0 и product REC=0. Каждые пять секунд
независимо обслуживает P1 cleanup и одну bounded retirement порцию; ошибки batch
не останавливают P1 TTL/CAS. HTTP не ждёт завершения глобального drain.
Прежние collector/recompute/funnel/returns workers из этого wiring удалены.

## Общий outbox и mixed RIL

Из required_consumers удаляются только recommendation-feedback-v1 и
flow-read-attribution-v1, включая новые inserts старого producer.
Пустой массив допустим. Если оставшихся обязанностей нет либо все выполнены,
закрывается только delivery metadata и снимается lease; fake ACK не создаётся.
Чужие невыполненные обязанности/lease остаются, чужие receipts не удаляются.
Собственные retired receipts запрещены на INSERT/UPDATE и удаляются bounded.

Старый RIL batch может содержать и anonymous, и retired компоненты.
ril_flush_flow принимает anonymous HTML, обнуляя два retired counters.
ril_flow_hourly INSERT ставит их нулевыми; UPDATE сохраняет OLD retired values.
Новые transition rows подавляются без отказа остальной транзакции.
Старые hourly/transition исторические значения остаются замороженными
до прежнего RIL TTL. Durable batch receipt/lease остаётся единственным replay gate.

SQL не стирает старую HMAC identity, sketches, path windows или process queue.
Release gate отдельно требует остановить все старые процессы, дождаться
согласованного DB drain и проверить новый бинарник. Штатный deploy мигрирует
до restart; полного online coexistence и rollback к старому collector нет.
Новый metadata/manual generation model — отдельный P2b2.

## Проверки и отдельные БД

Все проверки используют настоящие функции/triggers; production fences не
отключаются и не имеют тестового bypass.

| Назначение | БД / переменные | Target |
| --- | --- | --- |
| Историческая математика | tor_ci_legacy_analytics; LEGACY_ANALYTICS_DATABASE_URL, LEGACY_ANALYTICS_MIGRATOR_URL | legacy_analytics_baseline_pg, затем явно legacy suites |
| Сам переход и drained UI fixtures | tor_ci_collection_retirement_upgrade; COLLECTION_RETIREMENT_UPGRADE_DATABASE_URL, COLLECTION_RETIREMENT_APP_DATABASE_URL | collection_retirement_installed_upgrade_pg |
| Настоящий startup с неочищенным старым состоянием | tor_ci_collection_retirement_startup; COLLECTION_RETIREMENT_STARTUP_MIGRATOR_URL, COLLECTION_RETIREMENT_STARTUP_APP_DATABASE_URL | collection_retirement_startup_baseline_pg, затем flow_visits_http_pg startup |
| Текущие бизнес-отказы | tor_ci со всеми миграциями | current C/P1/catalog/HTTP/RIL suites |

Владелец disposable DB — tor_migrator; runtime — tor_app. Upgrade harness
проверяет имена и роли до записи, actual190 ledger/checksums, 191 без hooks,
ожидание in-flight старого writer, 192 twice. App DML re-grant не обходит fence.
Только этот disposable upgrade тест выдаёт TEMP для проверки temp-shadow;
production grants не расширяются.

Actual FK inventory, scope/account/book/outbox cascade, sorted daily locks,
busy batch, row-lock timeout, rollback/restart, два workers и числовой аудит
actual DELETE fan-out проверяются отдельно. Пограничный day-180 marker не теряется.
Mixed batch повторяется без double count, canonical/P1/чужой consumer сверяются точно.

Startup bootstrap создаёт actual190 fixtures и применяет192 twice, но не удаляет их.
Сам бинарник с AUTH=0 и всеми legacy flags=1 обязан начать HTTP и P1 cleanup
при busy batch, затем bounded drain; поздний настоящий C command остаётся
каноническим и не получает retired receipts.

Исходная P1 installed proof сохраняет собственную реальную границу189→190:
положительные старые consent assertions не выполняются под192.
Legacy191 report/browser suite не доказывает current erasure или startup.
Current UI unknown proof читает фактические drained fixtures отдельного upgrade
target: ci:retirement:linked, ci:retirement:untouched, ci:retirement:queued.
