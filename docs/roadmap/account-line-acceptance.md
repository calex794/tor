# Линия A — принятая реализация учётных записей и доступа

## Приёмка 31.10.2026

Основная линия принята в `main` через [№291](https://github.com/calex794/tor/pull/291), коммит `6979c8bf9008733df8ec703c88c97fdc59528b24`. Конечная ревизия ветки `d80f67db133d9ba1b00505e48f75789f3162d4c0` прошла [общую проверку](https://github.com/calex794/tor/actions/runs/41107177298) и [правила репозитория](https://github.com/calex794/tor/actions/runs/41107177309).

Список действующих устройств и отзыв другого/своего сеанса приняты через [№296](https://github.com/calex794/tor/pull/296), коммит `b9f0a4196ef57dd73259d0474020adb525ea3be8`. Конечная ревизия `beb0b708c09fed85f3e0b477738ed79ecbedda8c` прошла [PostgreSQL/HTTP/Chromium и остальные проверки](https://github.com/calex794/tor/actions/runs/41110636044), а также [правила репозитория](https://github.com/calex794/tor/actions/runs/41110635939).

**Граница решения владельца:** забытый пароль и реальная отправка восстановительного письма отложены. Код уже существует, но выключен по умолчанию. Описанные ниже испытания одноразового кода относятся к технической основе и тестовой почте; это не готовность реальной доставки и не поручение включать её. Яндекс остаётся заморожен. Сервер не обновлялся.

Source: NEwBOO `17125ef758a791e9630505c14d24f7fe65aed228`, `backend/app/web/routes/account_settings.py`; TOR `lanes/A.md`, `api-v1.md`, `access-authorization-contract.md`. Integration: [PR #291](https://github.com/calex794/tor/pull/291). This file maps implementation and acceptance commands; it does not substitute for exact-head CI evidence.

| Slice | Implementation / acceptance |
| --- | --- |
| A-01 / A-04 / A-10 | `contracts::auth` re-exports the established `paid_delivery::Actor` and positive ID, with server-derived `SessionActor`. Every existing web feature still resolves through one PG session port. No browser role/actor claim is deserialized. |
| A-02 / A-03 | Existing atomic registration and unique normalized email; Argon2id, bounded password jobs, hashed sessions (historically seven days; superseded by AUTH below), Origin/CSRF, explicit account state. Suspension revokes sessions permanently; reactivation does not resurrect cookies. Successful registration/login writes its bounded audit fact in the same transaction. |
| A-05 | `PgAccounts::change_account_role` requires a live administrator session, serializes grant/revoke and audits the change. The last active administrator cannot be revoked. Book/team roles and paid grants remain with their existing owners; global author/admin roles never populate reader entitlement facts. Initial administrator provisioning is an H/operator integration responsibility; there is no public self-grant endpoint. |
| A-06 | Private GET/POST `/me/settings`, GET/PATCH `/api/v1/me`: display name, normalized unique handle, existing adult opt-in and revision CAS. Unknown JSON fields are rejected. Delivery cadence reuses the C-17 table/port from #283 and its separate `/me/notification-settings` form; PATCH does not silently accept `delivery_settings`. Existing RIL privacy consent remains separately explicit and defaults off. No public profile is introduced. |
| A-07 | HTML and JSON password changes share one rotation command. Credential writes lock the account before sessions; login rechecks the verified hash under that same lock. Successful changes atomically create a replacement-session audit event with trusted peer IP, retained under the existing 30-day IP / 90-day event policy. A changed IP alone never blocks password rotation. `/forgot-password` and `/reset-password`: 256-bit secret, digest only in PG, 30-minute DB expiry, credential fingerprint, one-time atomic consumption, all previous sessions revoked, then explicit login. Email lookup/delivery is outside the identical bounded request-response path. |
| A-08 | Frozen by product decision. No Yandex/OAuth implementation or required acceptance. |
| A-09 | Existing PG identity and verified-peer attempt limits, bounded Argon2 workers; reset uses the same independent limits. Nginx real IP is trusted only when explicitly configured and the peer is loopback; forwarded claims are ignored. No email/token/IP metric labels. |
| A-11 | Активная часть принята по PostgreSQL/HTTP/Chromium в №291 и №296; BR-021 закрыта. S002 — поведение сверено; S004 остаётся частичным из-за отдельно отложенного восстановления забытого пароля. Точные ревизии и проверки приведены выше. |


## Приёмка AUTH 27.11.2026 — действующий контракт

Объединённый [PR #1326](https://github.com/calex794/tor/pull/1326) принят в
`main` коммитом `ea10b710aec187d863324456772ce9589d933361`.
Серия AUTH-02–08 заменяет прежний семидневный контракт A-03; историческая
приёмка №291/296 остаётся действительной для своего исходного объёма.
Промежуточные PR #1295, #1298, #1299 и #1303 закрыты как включённые в
объединённую реализацию, а не приняты независимо.

| Область | Действующий контракт |
| --- | --- |
| Срок сессии | 30 суток при входе, регистрации и ротации пароля; продление при успешной разрешённой активности не чаще одного раза в сутки. Фактический срок бездействия составляет 29–30 суток. |
| Активность | Успешная пользовательская навигация верхнего уровня same-origin и предусмотренные успешные операции сохранения позиции/завершения чтения; не фоновые запросы, polling, выдача S3-grant, отчёты или отклонённые команды. |
| Авторитет и хранение | PostgreSQL, непрозрачный 256-битный токен и SHA-256 digest; отзыв, истечение и неактивный аккаунт исключают продление. Прежние живые семидневные сессии могут перейти на новый срок без повторного входа. |
| Владелец | До 50 действующих сеансов, текущий первым, время окончания UTC, отзыв отдельного сеанса или всех остальных с подтверждением пароля; массовый отзыв не ограничен 50 строками интерфейса. |
| Администратор | Отзыв конкретного или всех сеансов другого аккаунта по живому полномочию и свежей проверке пароля; атомарный аудит. Без передачи браузером доверенного actor ID. |
| История | До 50 событий за 90 дней, включая собственный отзыв и проекцию административного аудита; без раскрытия токенов, IP или идентификатора администратора владельцу. |
| Критические операции | Обычное продление сессии не создаёт и не продлевает отдельный одноразовый proof для возвратов, внутренних выплат, передачи книги, долей и изменения S3-подключения. |
| Обычный выпуск | Сохранение действующих сессий при неизменной PostgreSQL, включая откат на предыдущую версию. |
| Восстановление | При потенциальной потере событий безопасности — изоляция, сверка состояния и, если непрерывность отзыва не доказана, явная инвалидизация восстановленных живых сеансов до публикации. |

Доказательства приёмки (точные ревизии и журналы):
- [Финальный Integration CI #1326](https://github.com/calex794/tor/actions/runs/45907661025) — 23/23; [FND-06](https://github.com/calex794/tor/actions/runs/45907660914) — пять этапов на двух разных бинарниках.
- [Post-merge Integration CI](https://github.com/calex794/tor/actions/runs/45910686836) — 23/23; [FND-06](https://github.com/calex794/tor/actions/runs/45910686661) — SUCCESS. Проверка правил старого SHA была отклонена после продвижения main; на следующем SHA правила и Integration CI прошли.
- [Integration CI актуального на момент записи main](https://github.com/calex794/tor/actions/runs/45914571294) (`9b3e108a81701e5175b121c48a80ed20d087bae5`) — 23/23; [Repository rules](https://github.com/calex794/tor/actions/runs/45914571268) — SUCCESS.
- AUTH-07: в PostgreSQL Foundation на последней ревизии 100 000 записей, пул 3, 6 работников, 432/432 операций, 0 ошибок; P95 2.730 / 19.180 / 15.865 мс (авторизация / первое продление / свежая сессия). Это измерение изолированной PG-фикстуры, не доказательство производственной пропускной способности.
- AUTH-08: в PostgreSQL Foundation получены `AUTH08B_STALE_BACKUP_REVOCATION_GAP_REPRODUCED`, `AUTH08C_WRONG_TARGET_REFUSED`, `AUTH08C_INVALIDATION_ROLLBACK_OK`, `AUTH08B_STALE_RESTORE_SESSION_INVALIDATION_OK`, `AUTH08_PG18_RESTORED_SESSIONS_OK`. Это CI логического восстановления, а не независимая эксплуатационная DR-приёмка.
- [AUTH-UI-01 #1315](https://github.com/calex794/tor/pull/1315) принят: единое окно входа/регистрации с резервным переходом без JavaScript. В reader-browser последнего CI подтверждён `TOR_AUTH_DIALOG_BROWSER_OK`. Строгий CSP платного контента не ослабляется; поддержка окна на строгом paywall и уникальный логин остаются отдельными задачами.

**Границы:** максимальный абсолютный срок сессии и отдельный TTL
администратора не утверждены. Инвалидизация при Recovery не заменяет сверку
потерянных изменений пароля, ролей, закрытия аккаунта и reset-proof.
Независимый восстановленный сервер, рабочая нагрузка с реальными читателями,
внешняя доставка почты и производственное развёртывание этими испытаниями не
подтверждены. Не изменять настройки восстановления пароля или внешней
доставки на основании этой записи; их актуальное состояние определяется
отдельной линией MAIL и операторской конфигурацией.

См. [AUTH-02](auth-02a-rolling-sessions.md), [AUTH-05c](auth-05c-integration.md),
[AUTH-06](auth-06b-s3-step-up.md), [AUTH-07](auth-07a-session-load.md),
[AUTH-08](auth-08a-session-continuity.md) и
[порядок независимого восстановления](../infra/i11-independent-dr-acceptance.md).

## Acceptance

The existing automatic `TOR integration CI` and manual PostgreSQL/browser workflows run the same new commands; no existing checks are removed:

- `cargo test -p tor-accounts --test credentials_pg -- --ignored --test-threads=1`: parallel rotations and stale verified login.
- `cargo test -p tor-accounts --test slices_pg -- --ignored --test-threads=1`: revision races, handle conflicts with rollback, adult flag versus grants, reset expiry/replay/parallel consumption/credential binding, role grant/revoke/audit/session denial.
- `cargo test -p tor-web --test password_change_http_pg a_07_ -- --ignored --test-threads=1`.
- `cargo test -p tor-web --test account_settings_http_pg -- --ignored --test-threads=1`: two actors, guest, unknown actor fields, Origin/CSRF, stale settings and shared JSON password rotation.
- `cargo test -p tor-accounts --test access_audit_pg -- --ignored --test-threads=1` and `cargo test -p tor-web --test account_access_audit_pg -- --ignored --test-threads=1`.
- `cargo test -p tor-reading --test notification_preferences_pg -- --ignored --test-threads=1`.
- `node tools/account_line_browser.cjs`: real server + PG + Chromium, two accounts, mobile/desktop settings, preference persistence, identical known/unknown reset response, private test-MTA capture, fragment removal, one-time reset and other-session rejection. Test mailbox accepts only `@example.invalid`; mail contents are not uploaded as artifacts.
- Existing Rust fmt/test/Clippy, schema/upgrade, ACL/paid-reader, browser and disaster-recovery suites remain required.

## Delivery and operating boundary

Recovery defaults off. Enabling requires `TOR_PASSWORD_RESET_ENABLED=1`, an absolute executable `TOR_RESET_SENDMAIL`, a safe `TOR_RESET_MAIL_FROM`, and the existing validated public origin. The adapter invokes only fixed `-i -t` arguments, writes a bounded message on stdin, suppresses child output and kills/reaps after five seconds. Token links use a URL fragment; the page clears it before form submission. Reset secrets never enter request URLs, RIL or audit records. Password changes remain available without mail configuration.

An accepted request is not proof of inbox delivery. The local MTA must durably queue accepted mail; end-to-end production delivery and MTA failure/restart monitoring require G/operator configuration and are not proven by the isolated test adapter. The application does not introduce another general-purpose outbox beside #277. A process crash before MTA handoff can lose a request; the reader can request another link. External notification cadence is only a saved preference, not a sender consent or active external newsletter.

Account-owned cleanup runs bounded batches every five minutes: IP cleared after 30 days, login events after 90 days, expired reset proofs after two days, attempt counters after one day. Outage backlogs drain on restart. Production retention/privacy policy and initial administrator provisioning still require operator review. No VPS deployment is authorized or performed by this PR.

## A-03 — владельцу видны и подконтрольны собственные сеансы (дополнительный пользовательский сценарий)

[Принятый PR №296](https://github.com/calex794/tor/pull/296): `GET /me/security/sessions` выводит максимум 50 действующих сеансов с временем входа UTC, семейством браузера и отметкой текущего устройства. В интерфейсе **нет** IP, хешей токенов, полного User-Agent или ложной «последней активности». Только владелец видит свои сеансы.

`POST /me/security/sessions` принимает проверенный формат UUID, действующий серверный сеанс и Origin/CSRF; PostgreSQL отзывает только принадлежащий владельцу сеанс, если действующий инициирующий сеанс всё ещё активен. Чужие/несуществующие ID дают одинаковый 404. Самоотзыв очищает cookie и требует новый вход. Нельзя менять другого пользователя и нельзя обойти защиту чужим UUID.

Проверка: `cargo test -p tor-web --test account_sessions_http_pg a_03_ -- --ignored --test-threads=1` на отдельной PG18; `node tools/account_line_browser.cjs` теперь проверяет завершение второй сессии реальным Chromium. Новые маршруты включены в RIL/ACL. Никакой GeoIP или автоматической блокировки по стране: на это нужны отдельные измерения и политика ложных срабатываний.
