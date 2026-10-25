# H — HTML интерфейс: готовая инструкция TOR

**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7733/h-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** A–G поставляют обработчики/use-case, I меняет общий роутер. H владеет только шаблонами/assets, бизнес-таблиц нет.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### H-01 — Д0–Д1

**Сделать:** Собрать маршруты всех S в таблицу page/form/role/api/status, согласовать единый shell и accessibility.

**Главный выходной файл:** `crates/web/src/pages/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages,templates,assets; routes I

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py)), [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /api/v1/books; POST /register; POST /login; POST /logout; POST /api/v1/books; POST /api/v1/purchases

**Проверка:** У каждого пользовательского действия есть форма и явно заданное состояние ошибки. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-02 — Д1 P0

**Сделать:** Главная → каталог → карточка → бесплатная глава; настоящие PostgreSQL данные без статичных заглушек.

**Главный выходной файл:** `crates/web/src/pages/catalog.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages catalog/reader

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py))

**Принятый API/действие:** GET /api/v1/books

**Проверка:** Chromium проходят гость и пользователь, рабочие mobile widths. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-03 — Д1–Д2 P0

**Сделать:** Регистрация/логин/профиль и безопасный redirect/CSRF формы через A.

**Главный выходной файл:** `crates/web/src/pages/auth.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/auth, templates

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout

**Проверка:** Cookie и submit работают без JS, чужой Origin отвергается сервером. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-04 — Д2 P0

**Сделать:** Автор: книга/глава, Markdown textarea, draft/preview/edit/version, опубликование.

**Главный выходной файл:** `crates/web/src/pages/editor.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/editor; API B

**Сценарий и старый код:** [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S022](https://app.notion.com/p/40432a84beec81eea8dbf2199500d3c7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py))

**Принятый API/действие:** POST /api/v1/books; POST /api/v1/books/{id}/chapters; PATCH /api/v1/chapters/{id}

**Проверка:** Размер главы <=50000 символов, сохранение/отмена, нет потерянных правок. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-05 — Д2 P0

**Сделать:** Читалка: оглавление 500+, controls prev/next, autosave, resume и paywall.

**Главный выходной файл:** `crates/web/src/pages/reader.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/reading; C/D ports

**Сценарий и старый код:** [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /reader/{chapter_id} (paywall); PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased; GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Нет зависания при большом оглавлении, автосохранение после reload. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-06 — Д3 P0

**Сделать:** Покупка: цена, ошибки остатка, подтверждение, библиотека купленной версии.

**Главный выходной файл:** `crates/web/src/pages/commerce.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/commerce; D API

**Сценарий и старый код:** [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/purchases

**Проверка:** Двойной клик не списывает дважды, запрещённая версия не показывается. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-07 — Д3–Д4 P1

**Сделать:** Команда/авторская аналитика, приглашения и отложенный выпуск.

**Главный выходной файл:** `crates/web/src/pages/teams.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/teams; B services

**Сценарий и старый код:** [S026](https://app.notion.com/p/40432a84beec8195a3f1e9d1409639ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S027](https://app.notion.com/p/40432a84beec81eea2d3ebdf8f1c11d4) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S058](https://app.notion.com/p/40432a84beec813cbae0dd569c0e2901) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/notifications.py))

**Принятый API/действие:** POST /api/v1/teams; POST /api/v1/teams/{id}/invites; POST /api/v1/chapters/{id}/schedule

**Проверка:** Другой пользователь не может изменять книгу даже через прямой HTTP. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-08 — Д4 P1

**Сделать:** Комментарии, подписки, клубы, личные сообщения, поддержка, notifications.

**Главный выходной файл:** `crates/web/src/pages/community.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/community; E/G

**Сценарий и старый код:** [S011](https://app.notion.com/p/40432a84beec812ca768e36d0445c06d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S012](https://app.notion.com/p/40432a84beec819abfd1c57e6c6da8ec) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py)), [S014](https://app.notion.com/p/40432a84beec81ecbfffda7f08feb751) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/users.py)), [S057](https://app.notion.com/p/40432a84beec816f8449df9a32040f38) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/mail.py))

**Принятый API/действие:** POST /api/v1/chapters/{id}/comments; POST /api/v1/comments/{id}/replies; GET /api/v1/me/inbox; POST /api/v1/users/{id}/follow; GET/POST /api/v1/me/messages

**Проверка:** Inbox и comment безопасно отображаются со спойлерами/markup. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-09 — Д4–Д5 P1

**Сделать:** Импорт и медиа UI с progress/status, отмена и сообщения ошибок.

**Главный выходной файл:** `crates/web/src/pages/import.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/imports; G

**Сценарий и старый код:** [S023](https://app.notion.com/p/40432a84beec817385d5fcd73e3d46f1) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py)), [S024](https://app.notion.com/p/40432a84beec8137a481c358f31df050) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapter_import.py)), [S025](https://app.notion.com/p/40432a84beec81ddb123ffc5dee763f0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py))

**Принятый API/действие:** POST /api/v1/import-jobs; POST /api/v1/import-jobs/{id}/upload; POST /api/v1/import-jobs (mode=url); POST /api/v1/books/{id}/cover

**Проверка:** Повтор формы не создаёт несколько import jobs. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-10 — Д5 P1

**Сделать:** Поиск/фильтры/жанры/SEO head; рекомендации и реклама только при подтверждённых данных.

**Главный выходной файл:** `crates/web/src/pages/search.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/pages/search; F

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py)), [S042](https://app.notion.com/p/40432a84beec81378f36e7e103841e6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/advertising/services/campaign_purchase_service.py)), [S043](https://app.notion.com/p/40432a84beec8126a746ee659e18bf02) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_advertising.py))

**Принятый API/действие:** GET /api/v1/books; POST /api/v1/ads/campaigns (draft-only); POST /api/v1/ads/rollout (DISABLED)

**Проверка:** Hidden/blocked не появляются в HTML, JS, sitemap. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-11 — Д5 P1

**Сделать:** Мобильный дизайн, клавиатура, focus, ошибка сети, escaping, CSP, responsive.

**Главный выходной файл:** `crates/web/assets/site.css` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web/assets, templates

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py)), [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /api/v1/books; POST /register; POST /login; POST /logout; GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Ввод UTF-16/IME и длинных глав без потери текста, XSS blocked. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### H-12 — Д5 P0

**Сделать:** Создать настоящую Chromium-suite для HTML цепочек гостя, читателя, автора, команды, модератора и админа по утверждённым URL; проверить HTTP/PG результат, direct-deny 404 и отсутствие секретного текста в body.

**Главный выходной файл:** `crates/web/tests/browser_slices.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** tests/browser (I подключает)

**Сценарий и старый код:** [S001](https://app.notion.com/p/40432a84beec81cbbc76f10f6635ee6d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/catalog.py)), [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S005](https://app.notion.com/p/40432a84beec8176ad60ff17f805cf20) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S006](https://app.notion.com/p/40432a84beec81bd9802d4e54f687b48) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S034](https://app.notion.com/p/40432a84beec8186b99dfdc62851fc6c) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S065](https://app.notion.com/p/40432a84beec8115b1f9f8567f855ce7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** GET /api/v1/books; POST /register; POST /login; POST /logout; GET /reader/{chapter_id} (paywall); PUT /api/v1/me/progress/free; PUT /api/v1/me/progress/purchased; POST /api/v1/books; POST /api/v1/books/{id}/chapters; POST /api/v1/purchases; GET /api/v1/chapters/{id}/read (deny)

**Проверка:** Роли guest/reader/author/mod/admin проверены на реальном PG. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-web --test slices_pg h_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
