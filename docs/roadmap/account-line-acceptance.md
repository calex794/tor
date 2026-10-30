# Active account line A — integration candidate

Source: NEwBOO `17125ef758a791e9630505c14d24f7fe65aed228`, `backend/app/web/routes/account_settings.py`; TOR `lanes/A.md`, `api-v1.md`, `access-authorization-contract.md`. Integration: [PR #291](https://github.com/calex794/tor/pull/291). This file maps implementation and acceptance commands; it does not substitute for exact-head CI evidence.

| Slice | Implementation / acceptance |
| --- | --- |
| A-01 / A-04 / A-10 | `contracts::auth` re-exports the established `paid_delivery::Actor` and positive ID, with server-derived `SessionActor`. Every existing web feature still resolves through one PG session port. No browser role/actor claim is deserialized. |
| A-02 / A-03 | Existing atomic registration and unique normalized email; Argon2id, bounded password jobs, hashed seven-day sessions, Origin/CSRF, explicit account state. Suspension revokes sessions permanently; reactivation does not resurrect cookies. Successful registration/login writes its bounded audit fact in the same transaction. |
| A-05 | `PgAccounts::change_account_role` requires a live administrator session, serializes grant/revoke and audits the change. The last active administrator cannot be revoked. Book/team roles and paid grants remain with their existing owners; global author/admin roles never populate reader entitlement facts. Initial administrator provisioning is an H/operator integration responsibility; there is no public self-grant endpoint. |
| A-06 | Private GET/POST `/me/settings`, GET/PATCH `/api/v1/me`: display name, normalized unique handle, existing adult opt-in and revision CAS. Unknown JSON fields are rejected. Delivery cadence reuses the C-17 table/port from #283 and its separate `/me/notification-settings` form; PATCH does not silently accept `delivery_settings`. Existing RIL privacy consent remains separately explicit and defaults off. No public profile is introduced. |
| A-07 | HTML and JSON password changes share one rotation command. Credential writes lock the account before sessions; login rechecks the verified hash under that same lock. `/forgot-password` and `/reset-password`: 256-bit secret, digest only in PG, 30-minute DB expiry, credential fingerprint, one-time atomic consumption, all previous sessions revoked, then explicit login. Email lookup/delivery is outside the identical bounded request-response path. |
| A-08 | Frozen by product decision. No Yandex/OAuth implementation or required acceptance. |
| A-09 | Existing PG identity and verified-peer attempt limits, bounded Argon2 workers; reset uses the same independent limits. Nginx real IP is trusted only when explicitly configured and the peer is loopback; forwarded claims are ignored. No email/token/IP metric labels. |
| A-11 | PG tests below plus real Chromium registration/settings/reset journey; Notion S002/S004 and BR-021 retain partial/in-review status until the final candidate SHA passes. |

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

[PR с реализацией](https://github.com/calex794/tor/pulls?q=is%3Apr+session-security-center): `GET /me/security/sessions` выводит максимум 50 действующих сеансов с временем входа UTC, семейством браузера и отметкой текущего устройства. В интерфейсе **нет** IP, хешей токенов, полного User-Agent или ложной «последней активности». Только владелец видит свои сеансы.

`POST /me/security/sessions` принимает проверенный формат UUID, действующий серверный сеанс и Origin/CSRF; PostgreSQL отзывает только принадлежащий владельцу сеанс, если действующий инициирующий сеанс всё ещё активен. Чужие/несуществующие ID дают одинаковый 404. Самоотзыв очищает cookie и требует новый вход. Нельзя менять другого пользователя и нельзя обойти защиту чужим UUID.

Проверка: `cargo test -p tor-web --test account_sessions_http_pg a_03_ -- --ignored --test-threads=1` на отдельной PG18; `node tools/account_line_browser.cjs` теперь проверяет завершение второй сессии реальным Chromium. Новые маршруты включены в RIL/ACL. Никакой GeoIP или автоматической блокировки по стране: на это нужны отдельные измерения и политика ложных срабатываний.
