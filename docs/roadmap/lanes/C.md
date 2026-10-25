# C — Читалка и библиотека: готовая инструкция TOR

**Дополнение 26.10 (PREF):** [16-case ACL и S3-grant](../access-authorization-contract.md), [RIL/progress](../contracts/events-analytics-v1.md), [refund exposure](../contracts/refunds-payouts-v1.md). Реальный paid V1 идёт S3→браузер; C даёт только metadata/grant и проверяет last readable block по PG manifest. Ни `grant`, ни S3 GET не считаются чтением.


**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7728/c-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Уточнение аудита 26.10:** единый reader gate должен исполнять [матрицу](../access-authorization-contract.md) для free/current, direct paid pinned V1, Layer CURRENT lifetime/30d, withdrawn, blocked/admin hold, refunded и cached direct asset URLs. Наличие `CanRead` одной главы не доказывает отсутствие обходов через preview/progress/TOC/SEO.

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** B сообщает разрешённую опубликованную версию, D — проверенный entitlement, A — Actor, H — читалку, I — PG и маршрутизацию.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### C-01 — · Д0–Д1

**Сделать:** Реализовать typed CanRead(viewer,edition) с Allowed(version) или Denied без тела; на HTTP-границе отобразить 401 для анонима без сессии, 403 для явной роли и 404 для приватного/непубличного контента.

**Главный выходной файл:** `crates/reading/src/services/access.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** contracts через I; reading/services/read_access

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall); GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Если entitlement неизвестен или сервис недоступен, текст не возвращается. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-02 — · Д1 P0

**Сделать:** Публичная читалка: only published/free текст с no-store, правильный Markdown/escaping.

**Главный выходной файл:** `crates/reading/src/api/reader.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/api, services/read, repositories/chapters (read projection)

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall); GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Гость читает опубликованную главу; draft, blocked, private отвергнуты прямым GET. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-03 — · Д1–Д2 P0

**Сделать:** TOC по курсору 500–1000 глав, детерминированная сортировка и переход prev/next.

**Главный выходной файл:** `crates/reading/src/repositories/toc.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/api/toc, indexes/order lookup

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall); GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Не теряются главы на границе страниц; bounded SQL, нет OFFSET/full scan/N+1. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-04 — · Д2 P0

**Сделать:** Прогресс свободного текста: сохранение точной редакции/позиции и монотонное обновление.

**Главный выходной файл:** `crates/reading/src/services/free_progress.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/progress, reading_positions

**Сценарий и старый код:** [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased

**Проверка:** Повтор/устаревший запрос не откатывает место; restart сохраняет позицию. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-05 — · Д2 P0

**Сделать:** Возобновление: login → previous chapter/position, смена опубликованной V1→V2 без ложного entitlement.

**Главный выходной файл:** `crates/reading/src/services/resume.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/history, version ID from B

**Сценарий и старый код:** [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased

**Проверка:** При возврате выбирается разрешённая версия; чужая позиция не видна. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-06 — · Д3 P0

**Сделать:** Купленная pinned-редакция: read via D AccessGrant, BKR-134 paid progress save/load.

**Главный выходной файл:** `crates/reading/src/services/paid_progress.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/progress_paid, contracts D entitlement

**Сценарий и старый код:** [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased

**Проверка:** Сохранить P у купленной V1, издать V2, вернуться к V1/P без повторной покупки. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-07 — · Д3 P0

**Сделать:** Paywall UX и безопасная причина отказа, без leak paid body/snippets/meta.

**Главный выходной файл:** `crates/reading/src/api/paywall.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/api/paywall; H экран

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall)

**Проверка:** Гостю предложен вход/покупка, в body/network/caches отсутствует текст. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-08 — · Д3–Д4 P1

**Сделать:** Личная библиотека: добавить/убрать книгу отдельно от purchased/library_progress, сортировка/пагинация.

**Главный выходной файл:** `crates/reading/src/services/library.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** library_items, reading/history

**Сценарий и старый код:** [S007](https://app.notion.com/p/40432a84beec814989c1e099bc020a34) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py))

**Принятый API/действие:** POST /api/v1/me/library/{book_id}

**Проверка:** Наличие bookmark не выдаёт платное право; пользователь Б не видит библиотеку А. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-09 — · Д4 P1

**Сделать:** Синхронизация прогресса на нескольких устройствах, stale update и перезапуск браузера.

**Главный выходной файл:** `crates/reading/src/services/sync.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** progress CAS/token/order policy

**Сценарий и старый код:** [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased

**Проверка:** Старый автосейв не переписывает новое положение; гонка детерминирована. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-10 — · Д4–Д5 P1

**Сделать:** Границы рендеринга: безопасный markdown, большие тексты, mobile, отсутствие утечки через ответ 404 и shared cache.

**Главный выходной файл:** `crates/reading/src/domain/renderer.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/renderer + security headers

**Сценарий и старый код:** [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Вредоносный HTML не выполняется, мобильная страница открывается и сохраняет progress. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-11 — · Д5 P0

**Сделать:** Сквозное соединение A+B+D+H: гость/покупатель/автор → опубликованная/купленная/скрытая глава.

**Главный выходной файл:** `crates/reading/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** I web wiring; reading suite

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall); PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased; GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Весь путь на реальной PostgreSQL+Chromium, прямые URL и negative access. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### C-12 — · Д5

**Сделать:** Сверка S005/S006/S007/S065, snapshot и поведение по NEwBOO.

**Главный выходной файл:** `crates/reading/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reading/tests, Notion матрица

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S007](https://app.notion.com/p/40432a84beec814989c1e099bc020a34) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall); PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased; POST /api/v1/me/library/{book_id}; GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Четыре S имеют ссылку на проверки и явно отмеченные несоответствия. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-reading --test slices_pg c_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
