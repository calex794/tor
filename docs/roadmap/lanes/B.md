# B — Книги, версии, команды: готовая инструкция TOR

**Дополнение 26.10 (PREF):** [private S3/immutable version](../contracts/import-storage-v1.md), [ACL/withdrawal](../access-authorization-contract.md), [RIL manifest/ChapterCompleted](../contracts/events-analytics-v1.md). B готовит ReaderVersionManifest и зашифрованный immutable object, publish только после HEAD/PG transaction, pinned V1 нельзя GC.


**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7727/b-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** Actor от A; данные версии/цены для C/D; событие публикует B в согласованный outbox I, G доставляет; F индексирует; H показывает формы.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### B-01 — · Д0–Д1

**Сделать:** Фиксировать инвариант: 1 произведение на (source_identity,target_language), BookId/ChapterId/VersionId, slug/visibility/lifecycle.

**Главный выходной файл:** `crates/books/src/domain/identity.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** books/domain, books/schemas, заявка I contracts

**Сценарий и старый код:** [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S020](https://app.notion.com/p/40432a84beec81cb8d79ff87ac8a395f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py))

**Принятый API/действие:** POST /api/v1/books; PATCH /api/v1/books/{id}

**Проверка:** Дубликат key отвергнут в домене и уникальным PG индексом. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-02 — · Д1 P0

**Сделать:** Создание книги и правка метаданных/жанров/языка/права видимости.

**Главный выходной файл:** `crates/books/src/services/books.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** api/books, services/books, repositories/books

**Сценарий и старый код:** [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py))

**Принятый API/действие:** POST /api/v1/books

**Проверка:** Создатель сохраняет книгу, гость не видит draft, чужой автор не редактирует. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-03 — · Д1–Д2 P0

**Сделать:** CRUD главы и редактор Markdown до 50 000 символов с правилом пустого/одинакового текста.

**Главный выходной файл:** `crates/books/src/services/chapters.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** api/chapters, services/chapter_edit, repositories/chapter_versions

**Сценарий и старый код:** [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/books/{id}/chapters

**Проверка:** После сохранения новая версия там, где нужно; >50k отклонено, пустой/неизменённый ввод проверен. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-04 — · Д2 P0

**Сделать:** Версии: immutable ChapterVersion, CAS/optimistic lock при конфликте редактирования.

**Главный выходной файл:** `crates/books/src/domain/versions.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** chapter_versions, expected_version, revision hash

**Сценарий и старый код:** [S022](https://app.notion.com/p/40432a84beec81eea8dbf2199500d3c7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py))

**Принятый API/действие:** PATCH /api/v1/chapters/{id}

**Проверка:** Параллельные правки не перетирают друг друга; старое содержимое не меняется. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-05 — · Д2 P0

**Сделать:** Отдельные состояния draft/published/archived/moderated, превью только разрешённому актору.

**Главный выходной файл:** `crates/books/src/services/visibility.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** services/visibility, repositories/books/chapters

**Сценарий и старый код:** [S020](https://app.notion.com/p/40432a84beec81cb8d79ff87ac8a395f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py)), [S022](https://app.notion.com/p/40432a84beec81eea8dbf2199500d3c7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py))

**Принятый API/действие:** PATCH /api/v1/books/{id}; PATCH /api/v1/chapters/{id}

**Проверка:** Прямой URL черновика/архива не выдаёт текст ни HTML, ни JSON. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-06 — · Д2–Д3 P0

**Сделать:** Публикация сейчас: атомарно версия + состояние + единственный outbox event и счётчики.

**Главный выходной файл:** `crates/books/src/services/publish.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** publish service; release_outbox; транзакция через I/G

**Сценарий и старый код:** [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S022](https://app.notion.com/p/40432a84beec81eea8dbf2199500d3c7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py))

**Принятый API/действие:** POST /api/v1/books/{id}/chapters; PATCH /api/v1/chapters/{id}

**Проверка:** Двойной publish не создаёт 2 события, откат не оставляет опубликованный текст. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-07 — · Д3 P0

**Сделать:** Отложенный выпуск: расписание, lease/fence worker, отмена/перенос, TZ и cron без дубликатов.

**Главный выходной файл:** `crates/books/src/services/schedule.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** publication_schedule + G/job contract

**Сценарий и старый код:** [S058](https://app.notion.com/p/40432a84beec813cbae0dd569c0e2901) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/notifications.py))

**Принятый API/действие:** POST /api/v1/chapters/{id}/schedule

**Проверка:** Два worker не выпускают главу дважды; отменённый выпуск не выходит. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-08 — · Д3 P1

**Сделать:** Команды: создать, пригласить, принять/отклонить, раздать роли.

**Главный выходной файл:** `crates/teams/src/services/invites.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** teams/domain/services/repositories; invites

**Сценарий и старый код:** [S026](https://app.notion.com/p/40432a84beec8195a3f1e9d1409639ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S027](https://app.notion.com/p/40432a84beec81eea2d3ebdf8f1c11d4) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py))

**Принятый API/действие:** POST /api/v1/teams; POST /api/v1/teams/{id}/invites

**Проверка:** Чужой пользователь не принимает чужое приглашение; дубликаты не создают повторных участников. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-teams --test slices_pg b_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-09 — · Д3–Д4 P1

**Сделать:** Привязать книгу к команде, роли редактора, передать владельца в атомарной транзакции.

**Главный выходной файл:** `crates/teams/src/services/transfers.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** book_members, team_books, transfer token/CAS

**Сценарий и старый код:** [S028](https://app.notion.com/p/40432a84beec81088797e00c339bfbbe) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S030](https://app.notion.com/p/40432a84beec81b99700fbd019d5c2c0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py))

**Принятый API/действие:** POST /api/v1/books/{id}/team; POST /api/v1/books/{id}/transfer

**Проверка:** При ошибке transfer прежнее владение сохраняется; две команды не становятся владельцами. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-teams --test slices_pg b_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-10 — · Д4 P1

**Сделать:** Версии авторских долей и snapshot распределения, передаваемый D в момент покупки.

**Главный выходной файл:** `crates/teams/src/domain/shares.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** revenue_share_versions; контракт D

**Сценарий и старый код:** [S030](https://app.notion.com/p/40432a84beec81b99700fbd019d5c2c0) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py))

**Принятый API/действие:** POST /api/v1/books/{id}/transfer

**Проверка:** После смены долей уже совершённая покупка сохраняет старую версию распределения. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-teams --test slices_pg b_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-11 — · Д4 P1

**Сделать:** Личные книги, приватные главы и разрешённое совместное чтение.

**Главный выходной файл:** `crates/books/src/services/personal.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** personal_book ACL; private book links

**Сценарий и старый код:** [S033](https://app.notion.com/p/40432a84beec812cbc09ccdf3ab902ca) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/private_books.py))

**Принятый API/действие:** POST /api/v1/me/private-books

**Проверка:** Приватная книга не попадает в каталог/SEO и не читается посторонним. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-12 — · Д4 P1

**Сделать:** Связанные книги, порядок глав и корректировка номера/slug без потери ссылок.

**Главный выходной файл:** `crates/books/src/repositories/toc.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** book_relations, chapter_order

**Сценарий и старый код:** [S020](https://app.notion.com/p/40432a84beec81cb8d79ff87ac8a395f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py)), [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** PATCH /api/v1/books/{id}; POST /api/v1/books/{id}/chapters

**Проверка:** Оглавление стабильно при 500+ главах, orphan ссылок не остаётся. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-13 — · Д4–Д5 P1

**Сделать:** Текстовые ошибки: автор получает заявку от E, правит версию, закрывает задачу с историей.

**Главный выходной файл:** `crates/books/src/services/text_correction.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** issue port E; books/service/correction

**Сценарий и старый код:** [S048](https://app.notion.com/p/40432a84beec8179922fea6ae510e93b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py))

**Принятый API/действие:** POST /api/v1/text-issues/{id}/resolve

**Проверка:** Закрытие не изменяет старую купленную версию и не теряет issue history. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_13_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-14 — · Д5 P1

**Сделать:** Аналитика команды/CSV с соблюдением прав и ограничением выборки.

**Главный выходной файл:** `crates/teams/src/services/analytics.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** teams/read models; export adapter

**Сценарий и старый код:** [S029](https://app.notion.com/p/40432a84beec8130bfe4e099bf7e49ef) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py))

**Принятый API/действие:** GET /api/v1/teams/{id}/analytics.csv

**Проверка:** Член другой команды не получает CSV; большой экспорт порционный. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-teams --test slices_pg b_14_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-15 — · Д5 P0

**Сделать:** Отдать C published+TOC, D immutable edition/price/share snapshot, F события каталога, H формы; общий сквозной путь.

**Главный выходной файл:** `crates/books/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** contracts через I, publish event G

**Сценарий и старый код:** [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S058](https://app.notion.com/p/40432a84beec813cbae0dd569c0e2901) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/notifications.py))

**Принятый API/действие:** POST /api/v1/books; POST /api/v1/books/{id}/chapters; POST /api/v1/chapters/{id}/schedule

**Проверка:** Создание → правка → выпуск → новый читатель в настоящем браузере, без заглушек. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_15_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### B-16 — · Д5

**Сделать:** Добавить в books/teams suite реальные PG/HTTP тесты для S019–S022,S026–S030,S033,S048,S058 и файл результатов со ссылками на исходные endpoint, различиями/разрешёнными изменениями и финальным SHA.

**Главный выходной файл:** `crates/books/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** tests/books + матрица Notion

**Сценарий и старый код:** [S019](https://app.notion.com/p/40432a84beec81d186aacb0203b3ecc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_detail.py)), [S020](https://app.notion.com/p/40432a84beec81cb8d79ff87ac8a395f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_settings.py)), [S021](https://app.notion.com/p/40432a84beec816d978bfa05775fd4cb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S022](https://app.notion.com/p/40432a84beec81eea8dbf2199500d3c7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_chapters.py)), [S026](https://app.notion.com/p/40432a84beec8195a3f1e9d1409639ab) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S027](https://app.notion.com/p/40432a84beec81eea2d3ebdf8f1c11d4) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S028](https://app.notion.com/p/40432a84beec81088797e00c339bfbbe) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py)), [S029](https://app.notion.com/p/40432a84beec8130bfe4e099bf7e49ef) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/dashboard_teams.py))

**Принятый API/действие:** POST /api/v1/books; PATCH /api/v1/books/{id}; POST /api/v1/books/{id}/chapters; PATCH /api/v1/chapters/{id}; POST /api/v1/teams; POST /api/v1/teams/{id}/invites; POST /api/v1/books/{id}/team; GET /api/v1/teams/{id}/analytics.csv; POST /api/v1/books/{id}/transfer; POST /api/v1/me/private-books; POST /api/v1/text-issues/{id}/resolve; POST /api/v1/chapters/{id}/schedule

**Проверка:** Для каждого S есть отдельный опыт и ссылка на код/SQL/CI или явно открытый пробел. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-books --test slices_pg b_16_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
