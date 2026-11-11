# REC/FLOW v2 — остановка индивидуального сбора и переход модели

## Основание и статус

Решение владельца в [существующей странице FLOW](https://app.notion.com/p/41132a84beec81c08561f999c74144fb),
верхний блок от 11.11.2026, полученный 10.11.2026 после изменения страницы в 20:36 UTC:
не добавлять отдельные запросы, переключатели или экраны согласия REC/FLOW;
сначала пересмотреть модель данных. Необоснованное индивидуальное отслеживание
не включается автоматически. Удаление формы не разрешает обходить прежние guards.

Этот договор заменяет runtime-предписания старых REC/FLOW контрактов только
в перечисленной области. Их миграции, сохранённые факты и результаты прежней
технической приёмки остаются историей. Сам этот документ не является приёмкой.
Нужны точный SHA, реальные PostgreSQL/HTTP/Chromium проверки и post-merge CI.
Формальный счётчик FLOW остаётся **1/9**, пока критерии целых этапов не выполнены.

| Пакет | Проверяемый результат | Что ещё не заявляется |
| --- | --- | --- |
| P1 | Явные скрытия книг, CAS и квитанции команд отделены от профиля аналитики | Отмена старого сбора |
| P2a | Новый сервер не собирает индивидуальные REC/FLOW/RIL данные, не потребляет их события; UI не просит согласие | Защита от старого бинарного процесса и полное физическое удаление |
| P2b1 | SQL запрещает старые записи; ограниченная очистка вычитает доказанный FLOW-вклад и инвалидирует неопределимые REC/organic показатели | Стирание чужой RAM, новая contextual model или завершение выпуска |
| P2b2 | Отдельная версия модели metadata/manual, явная граница build/activate/rollback | Повторное использование поведенческого legacy score под новым названием |
| Следующие FLOW | Минимизированные агрегаты времени события с явным источником и покрытием | Восстановление утраченной истории или обход нового решения |

## P2a: живой пользовательский путь

При `TOR_RECOMMENDATIONS_ENABLED=1` выдаются доступные новинки каталога:
`source=catalog`, `algorithm_version=catalog_new_releases_v1`,
`reason=new_release`, `personalized=false`, `generation_id=null`; билета нет.
`personalized=false` означает отсутствие подбора по истории. Проверка активного
аккаунта, текущего доступа и его явных P1-скрытий сохраняется. Явная seed-книга
также проходит текущий доступ. Ни предметный профиль, ни rollout hash,
ни последние чтения, ни старое смешанное поколение не участвуют в выдаче.

Это ограниченная выдача «Новинки», а не восстановленная полная модель content/manual
recommendations. Старые поколения могут включать behavioral/popularity даже для
гостя; их выборочное использование в P2a не разрешено. Публичные книги, поиск,
платные права, прогресс, библиотека и подписка на книгу сохраняют обычные правила.

Явное «Не интересно» и восстановление книги работают без аналитического
согласия по [P1](rec-explicit-preferences-v1.md): тот же namespace команд,
точный replay, CAS, 30-дневный TTL и самостоятельное обслуживание срока хранения.
P2a не вызывает массовый opt-out и не сбрасывает выбор пользователя.

| HTTP-поверхность | P2a поведение |
| --- | --- |
| GET рекомендаций и страницы | Продуктовый флаг сохраняется; безопасная выдача каталога и P1-выбор, без истории/билетов |
| POST REC/FLOW consent и `/me/behavior` | Прежние session/Origin/CSRF проверки, затем private 410 `COLLECTION_RETIRED`; без записи |
| POST impressions, recommendation activate, discovery activate | Те же охранные проверки, затем private 410; нет фиктивного подтверждения или перехода по билету |
| GET/POST feedback и HTML-формы P1 | Прежний действующий контракт, включая конфликт CAS/replay |
| POST управления legacy generation | Проверка актуальной роли/Origin/CSRF, затем private 410; refresh/activate/rollback не выполняются |
| GET legacy quality JSON | Проверка актуального администратора, затем private 410, а не неподписанный текущий массив |
| GET управления рекомендациями | HTML 200 после проверки администратора: ссылки на доступные авторские настройки и legacy Audience; без generation SQL и control forms |
| GET административной аудитории | Живые C-факты отдельно от явно помеченных сохранённых legacy-отчётов |

410 использует существующую форму ошибки:
`{"error":{"code":"COLLECTION_RETIRED","message":"…","request_id":"…"}}`.
Ответ 410 имеет `Cache-Control: no-store` и `X-Robots-Tag: noindex, nofollow`.
Нет входа — прежний 401; неверные Origin/CSRF или роль — прежний 403;
недоступная проверка сессии/роли — 503. Ошибки extractor и ограничения тела
сохраняются. Переход не ослабляет доступ к книгам, платному тексту и админке.
Статические assets могут сохранять URL, но не отправляют старые сигналы.

## Совместимость API

Это явное изменение семантического контракта **REC/FLOW v2**, а не незаметная
добавка к старому алгоритму. В OpenAPI revision `2026-11-11.v2` расширяется
response enum `RecommendationItem.source` значением `catalog`, а старые
операции сбора помечаются deprecated и получают 410 вместо прежнего успеха.
Клиенты с исчерпывающим старым enum должны обновиться. Существующие `/api/v1`
URL остаются совместимыми адресами отказа/новой выдачи; отдельный сервер или
дублирующий write-path не создаётся. Работающий legacy сбор ради обратной
совместимости не поддерживается: решение владельца требует его остановки.
Другие core API и их error/ACL/financial contracts этим не изменяются.

## Runtime и обслуживание

Продуктовый `TOR_RECOMMENDATIONS_ENABLED` остаётся валидируемым boolean;
значение 0 сохраняет отключённую рекомендательную поверхность.
Следующие прежние ключи не включают сбор и не блокируют запуск AUTH=0:
`TOR_REC_ATTRIBUTION_ENABLED`, `TOR_ORGANIC_ATTRIBUTION_ENABLED`,
`TOR_FLOW_VISITS_ENABLED`, `TOR_READING_RETURNS_ENABLED`,
`TOR_BOOK_READING_RETURNS_ENABLED`, `TOR_FLOW_PUBLIC_OPT_IN_ENABLED`,
`TOR_RIL_SINGLE_INSTANCE`; HMAC-ключ для retired RIL identity не читается.
Повторный запуск с прежними значениями 1 не возобновляет наблюдение.

HTTP не создаёт subject, assignment, touch, навигационный receipt или C scoped
capture proof. Вызовы C выполняются с отключёнными optional collection gates.
RIL не вычисляет HMAC-псевдоним и не передаёт subject в очереди path/sketch/window;
его числовые route/status/latency/SQL и допустимые HTML request counters остаются.

Production не запускает REC feedback consumer, graph rebuild или schedule refresh.
В частности, `tick_feedback(false,false)` запрещён: он всё равно создаёт delivery
receipt. Старый `funnel.cycle(false)` тоже не подходит: его reconciliation
читает новые canonical C факты. Используется только
`PgFlowFunnel::maintain_retired()`: финализация уже сохранённых flags,
неразрешённый D7 при expiry становится unknown; новых start/complete/return
связей нет. Ни один из двух retired consumers не подтверждает новые C события.

Один существующий supervisor на общем PostgreSQL pool работает также при
AUTH=0 и выключенном продуктовом REC-флаге. P1 preference TTL обслуживается
независимо от ошибок legacy housekeeping. Затем выполняются ограниченные
REC dirty-bucket recomputation/TTL, закрытие C return coverage и финализация
сохранённых flags, FLOW visit TTL, frozen funnel TTL и C scoped proof TTL.
Это обслуживание старых данных, не новый сбор и не гарантия непрерывного покрытия.
Долгие/ошибочные optional операции ограничены временем и повторяются позже;
они не запрещают основной HTTP-путь.

P2a сохраняет прежние сроки и атомарность. Часть dateless return markers,
REC scopes и ещё не истёкшие raw строки существует до P2b. Финализация старого
raw может дополнить исторический агрегат; opt-out/TTL/исправление может изменить
его по прежнему договору. Не обещать «всё стёрто» или неизменный архив в P2a.

## Значение административных чисел

| Данные | Допустимое описание |
| --- | --- |
| C canonical starts/completions | Принятые первые факты главы внутри указанного UTC-интервала |
| C `COUNT(DISTINCT user_id)` | Различные зарегистрированные аккаунты с такими фактами; не посетители и не ежедневная активность |
| RIL anonymous HTML requests | Наблюдённые допустимые HTML-запросы; не люди, сеансы или подтверждённое чтение |
| REC/organic source, F book/group, visits, funnel, D7/D30 | Сохранённые измерения прежней модели, `data-measurement="legacy-consented"`; текущий статус «Не измеряется» |
| RIL opt-in / visit starts | Текущее значение не выводится, статус «Не измеряется» |

C сохраняет личные бизнес-факты; агрегированный результат не делает обработку
`user_id` анонимной. В таблицах нет сквозного счётчика уникальных людей.
Старые окна подписываются как исторические записи в конкретных диапазонах,
а не текущая/предыдущая общая аудитория. Для retired-метрик не показывается
ложный текущий ноль. HTTP 200 исторического отчёта означает успешное чтение
сохранённых записей, а не работающий collector. Недоступное чтение остаётся 503.
Сохраняются live administrator guard и общий deadline 950 ms.

## P2b1: отдельный SQL и процессный барьер

Точный договор — [flow-collection-retirement-db-v1](flow-collection-retirement-db-v1.md).
Миграция 191 создаёт только примитивы; 192 атомарно подключает барьеры.
191 — техническая граница исторических тестов, не отдельный принятый production режим.

Approved C consent projections остаются row-lockable и безопасно пусты.
Optional capture callbacks становятся no-op, а INSERT/UPDATE в retired stores
и квитанции двух retired consumers запрещены. Canonical C и P1 не запрещаются.

FLOW вычитает только вклад удаляемых finalized raw через statement DELETE
transition tables. Scope helpers не вычитают повторно. Потерянный ещё сохраняемый
FLOW daily — ошибка целостности и rollback, а не clamp или ноль.
REC/organic не имеют точного materialization proof: атомарный anonymous marker
по source/UTC day/book делает соответствующие показатели неизвестными.
Отчёты распространяют неизвестность до итогов, долей и дельт до физического
удаления затронутого daily. Незатронутая история остаётся.

Один transaction advisory gate координирует batch, scope helpers и DELETE
родителей users/books/outbox до их row locks. Batch использует неблокирующую
попытку: busy означает `(0,true)`. Внутри остаются SKIP LOCKED, leaf-first,
сортировка aggregate keys и порядок book daily → visit daily.
Нельзя обещать отсутствие любых deadlock для произвольного старого caller,
который заранее заблокировал другие rows: timeout/rollback/retry обязательны.

После 192 always-on supervisor вызывает только независимый P1 preference TTL
и `retire_analytics_batch(256)`; прежние финализаторы/REC recompute/C returns
в production wiring больше не запускаются. HTTP не ждёт глобального drain.
Budget ограничивает суммарные DELETE/normalize/scrub work rows. Возвращённый
`erased_rows` считает только DELETE; ноль не означает завершённую очистку.

Из outbox снимаются только `recommendation-feedback-v1` и
`flow-read-attribution-v1`. Пустые obligations разрешены и закрывают delivery
metadata без fake ACK. Payload/ID/дедупликация и чужие obligations сохраняются.
Старый mixed RIL batch сохраняет route/SQL/anonymous HTML, а retired-компонент
подавляется; старые hourly значения заморожены и имеют прежний TTL.

**Все старые процессы и их очереди должны быть остановлены отдельно.**
SQL не стирает HMAC, path/window или очередь из чужой памяти.
Штатный deploy мигрирует до restart; полного online coexistence со старым
бинарником и возможности вернуть старый collector после барьера не заявляется.
`pending=false` — состояние ограниченной работы БД, не разрешение выпуска
и не доказательство остановленной RAM. Production deploy требует прежнего
отдельного разрешения и точного проверенного SHA.

## P2b2 и последующие FLOW

Новая metadata/manual модель, build/activate/rollback guard и очистка старых
generation pointers — отдельный следующий пакет. P2b1 сохраняет P2a catalog
fallback; старые поколения production serving не читает.

Новые anonymous aggregates не строятся переименованием персональных rows.
Без отдельной доказанной модели не обещаются источник визита до чтения,
unique visitors, физические сессии, индивидуальные воронки или возвраты.

## Приёмка и владельцы

I владеет общим wiring, контрактом/OpenAPI, миграциями P2b и правилом выпуска.
F — serving, frozen funnel maintenance и PG доказательствами.
C/A adapters — trusted producers, retired endpoints, business-preserving проверки.
H/root — административное представление истории и единый CI/browser пакет.

Обязательны реальные проверки на итоговом SHA:

- Старые flags=1, AUTH=0, product REC=0: сервер запускается. После 192 отдельная
  installed startup DB доказывает ограниченный drain и независимость P1 TTL
  от busy/failed batch. Новые canonical C facts не получают retired deliveries.
- Прежний enabled consent и свежий аккаунт: публичные/личные HTML не создают
  identity queue; valid retired POST даёт 410 без изменения row/epoch, нарушения
  Origin/CSRF/session/role по-прежнему отклоняются.
- Без согласий: каталог → новинки → «Не интересно» → восстановление, reload,
  exact replay, stale CAS, JS и no-JS; чужой/закрытый контент не раскрывается.
- Historical positive repositories и отчёты запускаются на отдельной disposable
  legacy191 DB с явным guard. Их результаты не доказывают current collection.
  Current192 проверки не отключают triggers и не создают новые retired rows;
  installed190→192 target доказывает сохранение/erasure существовавших данных.
- Администратор видит живые C/anonymous request counts и отдельно подписанную
  legacy-историю; обычный пользователь/гость не получают отчёт. Проверяются
  unavailable, пустое окно, UTC-границы, live role revocation и deadline.
- P2b1 дополнительно: fresh/installed SQLx twice с прежними checksum,
  old-writer waiting, app TEMP-shadow refusal, actual FK inventory, account/book/
  outbox DELETE races, two workers, полный budget включая fan-out и rollback.
  RIL mixed batch и сохранение canonical C/P1/чужих consumers обязательны.
- P2b2 отдельно доказывает model boundary/rollback. Выпуск всего перехода
  требует прекращения старых процессов и ограниченного согласованного drain.
  Прежний зелёный CI не является проверкой новых байтов.
