# A — Учётные записи и права: готовая инструкция TOR

**Дополнение 26.10 (PREF):** [права и единая матрица](../access-authorization-contract.md), [refund/S3 first exposure](../contracts/refunds-payouts-v1.md) — A выдаёт Actor и role/Origin/CSRF, но не право на кошелёк/reader только по auth; cross-feature PORТ I. Тесты ACL-01 и ACL-S3 обязательны до платной беты.


**Статус:** пооперационный *план реализации*, не отчёт о готовом коде. Связанные карточки [Tela](https://telawiki.com/spaces/430/pages/7726/a-main) и каноническая [матрица сценариев](https://app.notion.com/p/ea10f617944046b6bf37dc8473f41c2d). Весь код только в TOR. Источники: [NEwBOO 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228), [bokarust d11b27b431](https://github.com/calex794/bokarust/tree/d11b27b43125b93e59809147e8e687203fa0753b).

**Уточнение аудита 26.10:** Actor / роли нужны не только для логина, но для единой [матрицы CanDiscover/CanRead/CanPurchase/CanEdit/CanModerate](../access-authorization-contract.md). Модераторский просмотр через отдельный служебный путь не означает публичного права на blocked или purchased главу.

**Решения уже приняты:** [архитектура](../decisions-v1.md), [API](../api-v1.md), [схема PG, события и лимиты](../data-and-events.md), [режим тестов/фикстуры](../tests-and-handoffs.md). Фраза «сверить» в старой карточке означает **доказать соответствие выбранному контракту**, а не придумать новую архитектуру.

**Смежные линии:** I принимает Actor/UserId/contracts, B/C/D/E/H используют этот порт; G доставляет reset только при включённом токенном восстановлении.

**Пакет сдаётся** одной рабочей пользовательской вертикалью, финальным набором тестов и реальной интеграцией. На каждом промежуточном шаге исполнять только дешёвые format/check/contract проверки.

### A-01 — · Д0–Д1

**Сделать:** Создать в accounts типизированные DTO Register/Login/Session и Actor-проверку по точным полям исходных auth.rs; запрос на общие UserId/Actor передать I для единого contracts-crate, не делать второй auth-стек.

**Главный выходной файл:** `crates/contracts/src/auth.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** crates/accounts/src/domain и schemas; заявка I на UserId/Actor

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S003](https://app.notion.com/p/40432a84beec81f689c7f8dba2d9bcdb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S004](https://app.notion.com/p/40432a84beec819f8f64caf0bfc1d5aa) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/account_settings.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout; GET /auth/yandex/* (feature flag); PATCH /api/v1/me

**Проверка:** Список сохранённых и изменённых контрактов с источниками; без двойной системы сессий. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_01_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-02 — · Д1 P0

**Сделать:** Перенести регистрацию: нормализованный уникальный email, безопасный password hash, состояние аккаунта.

**Главный выходной файл:** `crates/accounts/src/services/register.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** api/register, services/register, repositories/users; users/credentials

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout

**Проверка:** Два пользователя регистрируются, дубликат отвергается, hash не попадает в HTML/log. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_02_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-03 — · Д1 P0

**Сделать:** Вход/выход: серверная сессия, TTL, отзыв, Secure/HttpOnly/SameSite cookie, Origin/CSRF.

**Главный выходной файл:** `crates/accounts/src/services/session.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** api/auth, services/session, repositories/sessions

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout

**Проверка:** Старая cookie после logout не работает; чужой Origin отвергнут. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_03_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-04 — · Д2 P0

**Сделать:** Определить Actor/Guest/roles, единый разрешающий порт для остальных областей.

**Главный выходной файл:** `crates/accounts/src/services/actor.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** contracts Actor/Permission через I; services/permissions

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout

**Проверка:** Поддельный actor_id и просмотр чужого кабинета запрещены. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_04_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-05 — · Д2 P0

**Сделать:** Ролевые ограничения автора/команды/администратора, изменение и отзыв роли с записью аудита.

**Главный выходной файл:** `crates/accounts/src/services/roles.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** repositories/roles/security_events

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout

**Проверка:** После отзыва старый запрос не проходит; роль автора не открывает платный текст. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_05_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-06 — · Д3 P1

**Сделать:** Настройки профиля: handle, privacy, 18+, delivery; обновление версии данных.

**Главный выходной файл:** `crates/accounts/src/services/profile.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** api/me; services/profile; account_settings

**Сценарий и старый код:** [S004](https://app.notion.com/p/40432a84beec819f8f64caf0bfc1d5aa) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/account_settings.py))

**Принятый API/действие:** PATCH /api/v1/me

**Проверка:** Изменения сохраняются; другая учётная запись не может их изменить; возрастное согласие не заменяет ACL. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_06_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-07 — · Д3 P1

**Сделать:** Смена пароля и восстановление подтверждённым одноразовым токеном; отзыв старых сессий.

**Главный выходной файл:** `crates/accounts/src/services/password.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** services/password; reset_tokens; событие доставки G

**Сценарий и старый код:** [S004](https://app.notion.com/p/40432a84beec819f8f64caf0bfc1d5aa) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/account_settings.py))

**Принятый API/действие:** PATCH /api/v1/me

**Проверка:** Просроченный/повторный токен недействителен, прежний пароль не работает. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_07_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-08 — ЗАМОРОЖЕНО (решение 30.10.2026)

**Не выполнять:** вход через Яндекс, OAuth, callback и связывание внешней личности до отдельной разморозки. Сохранено исключительно для исторической трассировки S003. Не включать в обязательную приёмку линии A. Смена и восстановление пароля A-07 и защита попыток A-09 остаются обязательными.

### A-08 — исторический план, не исполнять

**Сделать:** Яндекс OAuth: state/nonce/callback и связывание внешней личности по подтверждённому subject.

**Главный выходной файл:** `crates/accounts/src/services/yandex_oauth.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** external_identities; auth/yandex; точный HTTP подтвердить

**Сценарий и старый код:** [S003](https://app.notion.com/p/40432a84beec81f689c7f8dba2d9bcdb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** GET /auth/yandex/* (feature flag)

**Проверка:** Email чужой личности не связывает аккаунт; без внешних credentials — не считать готовым. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_08_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-09 — · Д4 P1

**Сделать:** Антибот и ограничение попыток, без раскрытия существования email.

**Главный выходной файл:** `crates/accounts/src/services/rate_limit.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** auth_attempts, policy/config; безопасные счётчики

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout

**Проверка:** Серия неверных попыток ограничена, персональные сведения не пишутся в метрики. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_09_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-10 — · Д5 P0

**Сделать:** Подключить A к единому Axum серверу, обновить вызовы B/C/D/E/H через опубликованный Actor.

**Главный выходной файл:** `crates/accounts/src/api/mod.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** api/router A; web wiring I

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S004](https://app.notion.com/p/40432a84beec819f8f64caf0bfc1d5aa) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/account_settings.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout; PATCH /api/v1/me

**Проверка:** В браузере один человек создаёт книгу, второй не редактирует чужую; один SHA. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_10_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

### A-11 — · Д5

**Сделать:** Сверка S002–S004: контракты, тексты отказов, внешние зависимости, тесты и ссылки.

**Главный выходной файл:** `crates/accounts/tests/slices_pg.rs` (создать либо дополнить, рядом допускаются профильные api/domain/repository модули того же владельца). **Данные/границы:** crates/accounts/tests; матрица Notion

**Сценарий и старый код:** [S002](https://app.notion.com/p/40432a84beec814e859ce86ea3ee0aea) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S003](https://app.notion.com/p/40432a84beec81f689c7f8dba2d9bcdb) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py)), [S004](https://app.notion.com/p/40432a84beec819f8f64caf0bfc1d5aa) ([исходник](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/account_settings.py))

**Принятый API/действие:** POST /register; POST /login; POST /logout; GET /auth/yandex/* (feature flag); PATCH /api/v1/me

**Проверка:** Для каждого S известны разрыв/PR/SHA/CI или документированный внешний блокер. Состояние/права подтвердить PG/HTTP или кодом при инфраструктурной задаче.

**Фильтр собственного теста (создать при реализации):** `cargo test -p tor-accounts --test slices_pg a_11_`. По окончании ветки — вся предметная suite на конечном SHA, затем интеграционный браузер/PG с I.

## Испытания Д6–Д15 и передача I

Д6–Д7: реальная PostgreSQL/миграции/ACL предметной области; Д8–Д9: Chromium и сравнение с NEwBOO; Д10–Д11: конкурентность/падение worker/retry/таймаут; Д12: полный предметный пакет на финальном SHA и интеграция; Д13–Д15: ограниченная бета, аудит исходных S, запись PR/SHA/CI в старую Notion-матрицу. Конкретные опасные сценарии и триггеры — в [tests-and-handoffs.md](../tests-and-handoffs.md).

Для каждого S указать один из статусов: доказан, частично доказан, заблокирован внешней системой, заморожен/исключён по продукту. **Зелёная компиляция — не статус готово.**
