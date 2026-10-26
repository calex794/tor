# F — Поиск и продвижение: готовая инструкция TOR

**Дополнение 26.10 (PREF):** [ADS-01…16 / два продукта](../ads-execution-contract.md), [точный FIN-ad расход](../contracts/finance-ad-exact-v1.md), [REC-01…14 + golden](../contracts/recommendations-golden-v1.md), [читательские события/RIL](../contracts/events-analytics-v1.md). Простой popularity сортировщик ≠ паритет, рекламная карточка ≠ organic recommendation. Изначально charged ads OFF.


**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7731/f-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Уточнение аудита 26.10:** в рекламе существуют две разные системы — [ADS-01…16](../ads-execution-contract.md), рекомендательный движок задаётся [REC-01…14](../recommendations-execution-contract.md). Старые F-10/F-11/F-07 — родительские задачи, не полное задание по деньгам/алгоритмам. Не объявлять скрытую/отключённую рекламу полноценным переносом.

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** B Published/visibility, E reactions, G outbox, D settlement, H SSR; F не владеет ledger.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### F-01 — Д0–Д1

**Сделать:** Реализовать PublicBookProjection из опубликованной книги с slug и единым predicate public AND NOT held/deleted; применять фильтр к SQL-каталогу, HTML/JSON, sitemap, suggestion и recommendation cache до пагинации.

**Главный выходной файл:** `crates/search/src/domain/public_projection.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** search/domain + contracts B/I

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py))

**Принятый API/действие:** GET /api/v1/books

**Проверка:** Нет скрытого материала в API, HTML, sitemap и recommendation cache. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-02 — Д1 P0

**Сделать:** Каталог: pagination/cursor, фильтры, сортировка, total при допустимой нагрузке.

**Главный выходной файл:** `crates/search/src/repositories/catalog.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** search/api, services/catalog, repositories/pg_query

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py))

**Принятый API/действие:** GET /api/v1/books

**Проверка:** 100k книг: bounded SQL, стабильный курсор, нет N+1/OFFSET горячего пути. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-03 — Д2 P0

**Сделать:** Поиск по title/author/genre/language с индексом PostgreSQL и безопасной нормализацией.

**Главный выходной файл:** `crates/search/src/repositories/full_text.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** search/services/search, repositories/index

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py))

**Принятый API/действие:** GET /api/v1/books

**Проверка:** Скрытая книга не обнаруживается через hint/suggest/snippet. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-04 — Д2 P1

**Сделать:** Главная и жанровые витрины/популярное/новое, версии кэша.

**Главный выходной файл:** `crates/search/src/services/showcases.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** discovery/projection, caches

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py))

**Принятый API/действие:** GET /api/v1/books

**Проверка:** Отзыв/скрытие книги исключает её из кэша и выдачи. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-05 — Д2–Д3 P1

**Сделать:** Рейтинги книг и переводов как разные показатели, pairwise reader ranking и оценки.

**Главный выходной файл:** `crates/recommendations/src/services/ranking.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** recommendations/ranking

**Сценарий и старый код:** [S008](https://app.notion.com/p/40432a84beec8104a828e8c88ea68d43) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/library.py))

**Принятый API/действие:** POST /api/v1/me/rankings/choice

**Проверка:** Повторы не меняют агрегат, exhausted/defer pair не навязывается. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-recommendations --test slices_pg f_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-06 — Д3 P1

**Сделать:** «Вселенная чтения»: книжная/пользовательская лента, видимость и generation token.

**Главный выходной файл:** `crates/recommendations/src/services/universe.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** recommendations/read_models

**Сценарий и старый код:** [S045](https://app.notion.com/p/40432a84beec81878958d45d0bf63875) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/api/v1/reading_universe.py))

**Принятый API/действие:** GET /api/v1/reading-universe

**Проверка:** Данные — производные, отзыв публикации отражается после invalidation. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-recommendations --test slices_pg f_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-07 — Д3–Д4 P1

**Сделать:** Рекомендательные модели: входные события чтения/оценки, пустой профиль, детерминизм.

**Главный выходной файл:** `crates/recommendations/src/services/recommend.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** recommendations/engine

**Сценарий и старый код:** [S044](https://app.notion.com/p/40432a84beec8134bab6ea44f2427ec8) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/home.py))

**Принятый API/действие:** GET /api/v1/books/{id}/recommendations

**Проверка:** Новый пользователь получает корректный fallback, приватные события не разглашаются. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-recommendations --test slices_pg f_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-08 — Д4 P1

**Сделать:** SEO: robots/sitemap/canonical/noindex, staged publish/rollback поколения.

**Главный выходной файл:** `crates/search/src/services/seo.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** search/seo, seo_generations

**Сценарий и старый код:** [S060](https://app.notion.com/p/40432a84beec81ee87a9fc86a48e371a) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/workers/serving_projection_worker.py))

**Принятый API/действие:** POST /api/v1/admin/seo-generations/{id}/activate

**Проверка:** Упавший rollout оставляет старую работоспособную карту; hidden не индексируется. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-09 — Д4 P1

**Сделать:** Реферальный invite+attribution; вознаграждение — вызов D, не собственная проводка.

**Главный выходной файл:** `crates/referrals/src/services/attribution.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** referrals/domain/services, attribution

**Сценарий и старый код:** [S052](https://app.notion.com/p/40432a84beec8165a970f2f67436ba82) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_sharing.py))

**Принятый API/действие:** GET /api/v1/me/referrals

**Проверка:** Нет self-referral/двойной выплаты/потери атрибуции. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-referrals --test slices_pg f_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-10 — Д4 P1

**Сделать:** Рекламные кампании и показы, раздельно approved/disabled, доказанный impression.

**Главный выходной файл:** `crates/search/src/services/ads.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** ads в линии F через регистрацию владельца I, events

**Сценарий и старый код:** [S042](https://app.notion.com/p/40432a84beec81378f36e7e103841e6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/services/campaign_purchase_service.py)), [S043](https://app.notion.com/p/40432a84beec8126a746ee659e18bf02) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_advertising.py))

**Принятый API/действие:** POST /api/v1/ads/campaigns (draft-only); POST /api/v1/ads/rollout (DISABLED)

**Проверка:** Отсутствующий provider и выключенный rollout не считаются живой рекламой. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-11 — Д5 P1

**Сделать:** Глубокая реклама и выплаты: антифрод/подтверждение показов/settlement через D.

**Главный выходной файл:** `crates/search/src/services/impressions.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** ads/repository, D/financial port

**Сценарий и старый код:** [S042](https://app.notion.com/p/40432a84beec81378f36e7e103841e6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/services/campaign_purchase_service.py)), [S043](https://app.notion.com/p/40432a84beec8126a746ee659e18bf02) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_advertising.py))

**Принятый API/действие:** POST /api/v1/ads/campaigns (draft-only); POST /api/v1/ads/rollout (DISABLED)

**Проверка:** Невозможен несуществующий показ, дублированный settlement и уход баланса в минус. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-12 — Д5 P2

**Сделать:** Читательские достижения и агрегированная аналитика без PII/raw text в метриках.

**Главный выходной файл:** `crates/recommendations/src/services/achievements.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** achievements, analytics_projection

**Сценарий и старый код:** [S046](https://app.notion.com/p/40432a84beec817da51ef15de3e4a165) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/api/v1/reader_progress.py))

**Принятый API/действие:** GET /api/v1/me/achievements

**Проверка:** Повтор события не удваивает счётчик, high-cardinality раскрытие запрещено. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-recommendations --test slices_pg f_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-13 — Д5 P0

**Сделать:** Сквозное событие B Published → G projection job → F catalog/SEO → H страница.

**Главный выходной файл:** `crates/search/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** I wiring + F integration suite

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py)), [S044](https://app.notion.com/p/40432a84beec8134bab6ea44f2427ec8) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/home.py)), [S060](https://app.notion.com/p/40432a84beec81ee87a9fc86a48e371a) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/workers/serving_projection_worker.py))

**Принятый API/действие:** GET /api/v1/books; GET /api/v1/books/{id}/recommendations; POST /api/v1/admin/seo-generations/{id}/activate

**Проверка:** После выпуска книга найдена, после скрытия исчезает, p95 query измерен. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_13_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### F-14 — Д5

**Сделать:** Составить проверяемую матрицу видеокреативов S068 по существующим метаданным книг с ссылками на источники; оформить как исследовательский документ, не production API.

**Главный выходной файл:** `crates/search/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** docs/roadmap/video-creative-matrix.md; только проверенные поля ISBN/язык/жанр/название при наличии

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py)), [S008](https://app.notion.com/p/40432a84beec8104a828e8c88ea68d43) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/library.py)), [S042](https://app.notion.com/p/40432a84beec81378f36e7e103841e6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/services/campaign_purchase_service.py)), [S043](https://app.notion.com/p/40432a84beec8126a746ee659e18bf02) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_advertising.py)), [S044](https://app.notion.com/p/40432a84beec8134bab6ea44f2427ec8) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/home.py)), [S045](https://app.notion.com/p/40432a84beec81878958d45d0bf63875) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/api/v1/reading_universe.py)), [S046](https://app.notion.com/p/40432a84beec817da51ef15de3e4a165) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/api/v1/reader_progress.py)), [S052](https://app.notion.com/p/40432a84beec8165a970f2f67436ba82) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_sharing.py))

**Принятый API/действие:** GET /api/v1/books; POST /api/v1/me/rankings/choice; POST /api/v1/ads/campaigns (draft-only); POST /api/v1/ads/rollout (DISABLED); GET /api/v1/books/{id}/recommendations; GET /api/v1/reading-universe; GET /api/v1/me/achievements; GET /api/v1/me/referrals; POST /api/v1/admin/seo-generations/{id}/activate; docs/video-creative-matrix (PR-only research)

**Проверка:** Каждая строка содержит source URL и проверенный факт; PR-only S068 не объявляется функцией действующего NEwBOO main. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-search --test slices_pg f_14_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## AUTHOR — read-only статистика книги, 26.10.2026

**F-12/G-10/B-14:** для [AUTHOR-01…08](../author-workspace-execution-contract.md) сформировать безопасные агрегаты просмотров и статистики по датам/главам, без платного текста или персональных данных. **Деньги только из D ledger**, без двойного начисления при повторе; `GET` не запускает полный пересчёт. Подробная приёмка включает 366-дневный предел, `top_limit 1..50`, chapter ownership, пустую книгу и финальный `data_as_of`. Эти 8 маршрутов автора не смешивать с командной аналитикой S029.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
