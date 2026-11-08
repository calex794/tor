# E — Сообщества и модерация: готовая инструкция TOR

**Дополнение 26.10 (PREF):** [ACL social/moderation deny](../access-authorization-contract.md), [trusted events/privacy](../contracts/events-analytics-v1.md), [system disposition money](../contracts/refunds-payouts-v1.md). Модератор сообщает reason и переводит hard deny; D выполняет отдельно урегулирование денег, E не правит ledger. Нельзя утечь private/paid цитатой из Inbox/соцсигналов.


**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7730/e-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** A авторизация, B/C доступность target, G delivery, H формы; E не изменяет книги/деньги.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### E-01 — Д0–Д1

**Сделать:** Реализовать Comment, DiscussionThread, InboxEntry, UserRelation и ModeratorScope как типы E-модулей, с единым CommentId/anchor/visibility и запретом копирования приватной цитаты в Inbox; миграции передать I.

**Главный выходной файл:** `crates/discussions/src/domain/comments.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** contracts I; social/domain, discussion schemas

**Сценарий и старый код:** [S011](https://app.notion.com/p/40432a84beec812ca768e36d0445c06d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S012](https://app.notion.com/p/40432a84beec819abfd1c57e6c6da8ec) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py)), [S013](https://app.notion.com/p/40432a84beec81f6aa0bc0972b8482af) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py))

**Принятый API/действие:** POST /api/v1/chapters/{id}/comments; POST /api/v1/comments/{id}/replies; GET /api/v1/me/inbox; POST /api/v1/chapters/{id}/comments (spoiler)

**Проверка:** Каждый комментарий имеет единственный authoritative id. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-02 — Д1–Д2 P1

**Сделать:** Создание/редактирование/удаление комментария к доступной версии главы и якорю фрагмента.

**Главный выходной файл:** `crates/discussions/src/services/comments.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** discussions/api,services,repositories; comments,anchors

**Сценарий и старый код:** [S011](https://app.notion.com/p/40432a84beec812ca768e36d0445c06d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/chapters/{id}/comments

**Проверка:** Нет комментария к недоступной/чужой версии; anchor валидирован. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-03 — Д2 P1

**Сделать:** Ответы, дерево, упоминания и единый Inbox без второй независимой ветки обсуждения.

**Главный выходной файл:** `crates/discussions/src/services/inbox.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** threads,replies,inbox_refs,read_state

**Сценарий и старый код:** [S012](https://app.notion.com/p/40432a84beec819abfd1c57e6c6da8ec) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py))

**Принятый API/действие:** POST /api/v1/comments/{id}/replies; GET /api/v1/me/inbox

**Проверка:** Входящее сообщение ссылается на оригинальный Comment, повтор доставки без дубля. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-04 — Д2 P1

**Сделать:** Спойлеры, безопасный markdown, реакции/спасибо и защита приватных цитат.

**Главный выходной файл:** `crates/discussions/src/services/spoilers.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** discussion renderer и spoiler policy

**Сценарий и старый код:** [S013](https://app.notion.com/p/40432a84beec81f6aa0bc0972b8482af) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py))

**Принятый API/действие:** POST /api/v1/chapters/{id}/comments (spoiler)

**Проверка:** Спойлер скрыт в email/inbox/preview; HTML injection нейтрализована. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-05 — Д2–Д3 P1

**Сделать:** Подписки на автора/книгу, отписка, follow activity через G-release event.

**Главный выходной файл:** `crates/social/src/services/follow.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** subscriptions,notification_prefs

**Сценарий и старый код:** [S014](https://app.notion.com/p/40432a84beec81ecbfffda7f08feb751) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/users.py)), [S018](https://app.notion.com/p/40432a84beec8166a39dd644a0ef3fc7) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/account_settings.py))

**Принятый API/действие:** POST /api/v1/users/{id}/follow; GET /api/v1/me/inbox; PATCH /api/v1/me/notification-settings

**Проверка:** Отписавшийся не получает новые уведомления; подписка не даёт платный доступ. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-social --test slices_pg e_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-06 — Д3 P1

**Сделать:** Дружба, блокировки, профиль, рекомендация книги другу; настройки видимости.

**Главный выходной файл:** `crates/social/src/services/relationships.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** relationships,friend_requests,blocks

**Сценарий и старый код:** [S015](https://app.notion.com/p/40432a84beec81f2b678f94ec4adae3b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/users.py)), [S016](https://app.notion.com/p/40432a84beec815784adf00886f6be7a) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/library.py))

**Принятый API/действие:** POST /api/v1/users/{id}/block; POST /api/v1/books/{id}/share

**Проверка:** Блокировка запрещает новые сообщения и раскрытие приватных обновлений. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-social --test slices_pg e_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-07 — Д3 P1

**Сделать:** Коллекции, рейтинг/лайк, удаления рекомендаций из социальной библиотеки.

**Главный выходной файл:** `crates/social/src/services/collections.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** collections,collection_items,reactions

**Сценарий и старый код:** [S009](https://app.notion.com/p/40432a84beec819d8e01feea186ee866) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/collections.py)), [S010](https://app.notion.com/p/40432a84beec81f2864dee36680dc46e) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_interactions.py)), [S017](https://app.notion.com/p/40432a84beec81d3acffd4a6587e070f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/library.py))

**Принятый API/действие:** POST /api/v1/me/collections; POST /api/v1/books/{id}/reactions; DELETE /api/v1/me/recommendations/{id}

**Проверка:** Владелец редактирует только свои подборки; лайк не дублируется. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-social --test slices_pg e_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-08 — Д3–Д4 P1

Сверка с исходником и незакрытые разрывы: [перенос личной почты S057](../e08-private-mail.md). PR остаётся Draft до подтверждения исходного пользовательского пути; отдельные работающие маршруты не закрывают паритет.

**Сделать:** Личная переписка, сообщения, polling/unread, приватные ссылки на книги.

**Главный выходной файл:** `crates/social/src/services/messages.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** conversations,messages,last_seen

**Сценарий и старый код:** [S057](https://app.notion.com/p/40432a84beec816f8449df9a32040f38) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/mail.py))

**Принятый API/действие:** GET/POST /api/v1/me/messages

**Проверка:** Пользователь третий не может читать диалог; скрытая книга не раскрыта превью. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-social --test slices_pg e_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-09 — Д4 P2

**Сделать:** Клубы: приглашения, членство, контент, роли, превращение обсуждения в candidate club.

**Главный выходной файл:** `crates/clubs/src/services/membership.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** clubs,memberships,club_factory

**Сценарий и старый код:** [S063](https://app.notion.com/p/40432a84beec81e88b7bcda7f8f3597f) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py)), [S064](https://app.notion.com/p/40432a84beec8110a28bf93e4a13525d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/api/v1/clubs.py))

**Принятый API/действие:** POST /api/v1/clubs/from-thread; POST /api/v1/clubs/{id}/join

**Проверка:** Состояния членства и допуск проверены, частичный исходный scope не объявлен полным. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-clubs --test slices_pg e_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-10 — Д4 P1

**Сделать:** Жалобы, модерация главы/книги/пользователя, протокол решений/апелляций.

**Главный выходной файл:** `crates/support/src/services/moderation.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** reports,moderation_actions,restriction_state

**Сценарий и старый код:** [S049](https://app.notion.com/p/40432a84beec8133a22adcd31b6559e4) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/moderation.py))

**Принятый API/действие:** POST /api/v1/reports; POST /api/v1/admin/reports/{id}/decide

**Проверка:** Модератор клуба не может действовать как global admin. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-support --test slices_pg e_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-11 — Д4 P1

**Сделать:** Поддержка: обращения гостя, привязка после входа, операторская переписка.

**Главный выходной файл:** `crates/support/src/services/tickets.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** support_tickets,support_messages,guest_claims

**Сценарий и старый код:** [S050](https://app.notion.com/p/40432a84beec813f925ee4c43861b302) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S051](https://app.notion.com/p/40432a84beec8102b267c9ee50235939) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/admin_support.py))

**Принятый API/действие:** POST /api/v1/support/tickets; POST /api/v1/support/tickets/{id}/messages

**Проверка:** Нельзя захватить тикет чужой сессией; повтор события безопасен. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-support --test slices_pg e_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-12 — Д5 P1

**Сделать:** Сообщение об ошибке текста и ответ автора в B без прямой записи E в версии.

**Главный выходной файл:** `crates/discussions/src/services/text_issues.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** text_issue, published IssueResolved event

**Сценарий и старый код:** [S047](https://app.notion.com/p/40432a84beec81c7b8b3e96b2bbe53f8) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py))

**Принятый API/действие:** POST /api/v1/text-issues

**Проверка:** Исправление не стирает ранее купленный текст; закрытие дублированной заявки идемпотентно. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_12_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-13 — Д5 P1

**Сделать:** Подключить E к A/B/C/G/H, единый HTTP и браузер обсуждения/уведомлений.

**Главный выходной файл:** `crates/discussions/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** web via I, integration suite

**Сценарий и старый код:** [S011](https://app.notion.com/p/40432a84beec812ca768e36d0445c06d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S012](https://app.notion.com/p/40432a84beec819abfd1c57e6c6da8ec) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py)), [S049](https://app.notion.com/p/40432a84beec8133a22adcd31b6559e4) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/moderation.py))

**Принятый API/действие:** POST /api/v1/chapters/{id}/comments; POST /api/v1/comments/{id}/replies; GET /api/v1/me/inbox; POST /api/v1/reports; POST /api/v1/admin/reports/{id}/decide

**Проверка:** Два читателя → comment/reply/mention → inbox → жалоба → решение. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_13_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### E-14 — Д5

**Сделать:** Закрыть матрицу S009–S018,S047,S049–S051,S057,S063–S064.

**Главный выходной файл:** `crates/discussions/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** Notion matrix, source links

**Сценарий и старый код:** [S009](https://app.notion.com/p/40432a84beec819d8e01feea186ee866) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/collections.py)), [S010](https://app.notion.com/p/40432a84beec81f2864dee36680dc46e) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/books_interactions.py)), [S011](https://app.notion.com/p/40432a84beec812ca768e36d0445c06d) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_page.py)), [S012](https://app.notion.com/p/40432a84beec819abfd1c57e6c6da8ec) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py)), [S013](https://app.notion.com/p/40432a84beec81f6aa0bc0972b8482af) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/reader_comments.py)), [S014](https://app.notion.com/p/40432a84beec81ecbfffda7f08feb751) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/users.py)), [S015](https://app.notion.com/p/40432a84beec81f2b678f94ec4adae3b) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/users.py)), [S016](https://app.notion.com/p/40432a84beec815784adf00886f6be7a) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/library.py))

**Принятый API/действие:** POST /api/v1/me/collections; POST /api/v1/books/{id}/reactions; POST /api/v1/chapters/{id}/comments; POST /api/v1/comments/{id}/replies; GET /api/v1/me/inbox; POST /api/v1/chapters/{id}/comments (spoiler); POST /api/v1/users/{id}/follow; POST /api/v1/users/{id}/block; POST /api/v1/books/{id}/share; DELETE /api/v1/me/recommendations/{id}; GET /api/v1/me/inbox; PATCH /api/v1/me/notification-settings; POST /api/v1/text-issues; POST /api/v1/reports; POST /api/v1/admin/reports/{id}/decide; POST /api/v1/support/tickets; POST /api/v1/support/tickets/{id}/messages; GET/POST /api/v1/me/messages; POST /api/v1/clubs/from-thread; POST /api/v1/clubs/{id}/join

**Проверка:** Нет выпавших S, даже если поведение текущего NEwBOO частично. Реальная PG обязательна, HTTP и права сравнить с исходником.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-discussions --test slices_pg e_14_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## AE-E09 — комментарии к книге (3 самостоятельных маршрута)

[Конкретный сценарий и проверки](../domains-a-e-parity-2026-10-26.md) дополняют E-02/E-03/G-07/H-08: `GET /books/{ref}/fragments/comments`, `POST /books/{ref}/comments`, `POST /books/{ref}/comments/{comment_id}/delete` — это комментарии к **книге**, не к главе, подборке, клубу или платному Layer. В source есть отдельные `comment_intent_key` (защита повтора), `parent_comment_id`, режим видимого комментария и отправка side-effects лишь после успешного создания; удаление доступно владельцу книги по проверенной политике. E хранит общий Thread target `book`, не заводит второй движок комментариев. Обязательны A actor+CSRF, B visibility, G idempotent outbox, H fragment, отрицательные проверки чужой книги/пользователя/удалённого комментария, 2 worker и PG+HTTP+Chromium. Сборщик F021/S024 остаётся заморожен.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
