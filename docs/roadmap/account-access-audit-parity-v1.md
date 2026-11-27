# NEwBOO → TOR: атрибуция IP, журнал входов и аномалии сессий

**ACCESS-AUDIT-01…06 — детализация A-03/A-09, I-05/I-13, H-03/H-12, S002/S004.**
Дата сверки: 29.10.2026. **Это план/аудит, не включённая функция TOR.** Задачи
A–I и 69 сценариев не перенумеровываются; отдельной «системы слежения» не создаём.

## Что реально есть у источника

- [NEwBOO: user_access_audit_service.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/user_access_audit_service.py)
  вызывается из `_finalize_successful_auth` в
  [web/routes/auth.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py).
  При подтверждённом входе best-effort записываются `user_id`,
  trusted-proxy-safe `ip_address`, ограниченный `user_agent`,
  `event_type`, timestamp и bounded RIL route context;
  токены/cookies в аудит не передаются.
- [NEwBOO: YDB user_access_audit_repository.py](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/repositories/ydb/user_access_audit_repository.py)
  предоставляет `user_login_events` (с поиском по user_id/IP/time)
  и чтение `user_known_ips` (first_seen_at, last_seen_at, login_count,
  last_user_agent). **Обнаружен неполный контур:** в YDB
  `record_successful_login` в прочитанном коде пишет только событие;
  обновление `user_known_ips` в этом методе отсутствует. В
  [memory-реализации](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/repositories/user_access_audit_repository.py)
  оно есть. Не объявлять YDB known-IP агрегат корректно обновляемым
  без отдельного runtime/producer доказательства.
- [NEwBOO: user_sessions](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/repositories/ydb/async_user_session_repository.py)
  хранит jti, user_id, created/expires/revoke, **не** исходный IP,
  страну или устройство. В /me/settings показываются действующие
  сессии и отметка текущей, но IP к каждой не присоединён. Журнал
  входов сам по себе **не** доказывает привязку к сессии.
- [TOR: sessions DDL](../../migrations/202610120001_create_accounts.sql)
  хранит token_hash/user_id/timestamps/revoked_at без IP/UA/geo/last_seen.
  [TOR: auth_attempts DDL](../../migrations/202610120003_auth_attempts.sql)
  содержит только необратимые hashed counters.
  [TOR: login_limiter.rs](../../crates/web/src/auth/login_limiter.rs)
  читает trusted peer IP для rate-limit, но не сохраняет его рядом
  с аккаунтом; [account_form_http.rs](../../crates/web/src/auth/account_form_http.rs)
  создаёт сессии без network metadata. **Паритет не реализован.**

## Целевой контракт

| Этап | Владелец | Что добавить | Граница приёмки |
| --- | --- | --- | --- |
| ACCESS-AUDIT-01 — session identity | A-03 / I-05 | Безопасный внутренний session UUID (не raw cookie/token); `sessions.user_id, created_at, last_seen_at, revoked_at` и связь с записью об успешной регистрации/входе. Не приписывать failed login неустановленному user_id. | Две сессии одного пользователя остаются различимыми после restart; чужой токен не сопоставляет сессию. |
| ACCESS-AUDIT-02 — IP/UA capture | A-03/A-09 | На успешной аутентификации привязать проверенный edge IP (PostgreSQL `inet` с ограниченным доступом), bounded UA/device family и время. Сохранить IP при входе и отдельно редкие последующие network transitions, не делать PG UPDATE на каждый GET. | Поддельные Forwarded/X-Real-IP отвергнуты; Nginx loopback не записывается как адрес всех пользователей; IPv6 и несколько устройств проверены. |
| ACCESS-AUDIT-03 — append + known-IP | A-09 / I-05 | `account_login_events` с `user_id,session_id,ip,captured_at,ua_family,event_kind` и `account_known_ips` с first/last/count **либо** bounded projection; единый idempotent PG write/upsert. Donor YDB update-gap не копировать. | После двух входов + restart журнал и агрегат согласованы; не создаются дубликаты при повторном событии; сбой enrichment не теряет login. |
| ACCESS-AUDIT-04 — geo enrichment | I-05 / A-09 | Отдельное асинхронное отображение `IP -> country/ASN` с versioned local GeoIP snapshot, возможностью `unknown` и обновляемой геобазой; никакого внешнего API на каждом входе. City optional, точную геолокацию не обещать. | Нет блокировки при ошибке/отсутствии GeoIP; IPv4/IPv6; результат фиксирует версию базы и степень уверенности. |
| ACCESS-AUDIT-05 — anomaly policy | A-09 / I-13 | Режим только предупреждений: первая новая страна, новая сеть+UA, существенная гео-несовместимость двух **подтверждённых** событий, входы из необычных ASN, одновременно активные сессии/сетевые переходы. Нормальные VPN, CGNAT, роуминг и общий IP учитываются; страна/IP не являются доказательством взлома. | Нет автоматического бана только за новую страну или 2 сессии; риск объясним, ложноположительное событие можно закрыть. |
| ACCESS-AUDIT-06 — user control/privacy | H-03/H-12 / A-03 | /me/security: последние входы и активные сессии с датой, страной (если установлена), частично замаскированным IP и UA family, current badge; отзыв выбранной сессии и «выйти со всех других», optional уведомление о рискованном входе/step-up. TTL, delete/anonymization и role gate для IP security data. | User A не видит события user B; при revoke token сразу непригоден; сохранение raw IP ограничено сроком; нет raw IP/email в Prometheus/RIL labels. |

## Решения по стоимости и ложным блокировкам

1. Для VPS 1 vCPU / ~1 GiB RAM: одна малая PG запись при успешном
   login/register; `last_seen` обновлять не чаще раз в 10–15 минут/сессию
   (или при существенной смене IP), а GeoIP считать на login/asynchronous
   worker. Нет запроса к чужому API или записи на каждый просмотр главы.
2. Сочетание разных стран и одновременных сессий — **сигнал**, не verdict.
   Нельзя ошибочно запретить покупателю доступ из-за VPN, роуминга,
   shared Wi-Fi, домашнего сервера или неверной GeoIP-классификации.
3. Предлагаемый первый режим: сбор события → сравнение с известными
   сетями → журнал риска → выборочное оповещение/step-up при нескольких
   независимых признаках. Никакого автоматического постоянного IP-ban.
4. Безопасность аккаунтов — отдельный домен от RIL behavior consent.
   Для IP/UA events нужны документированные цели, сроки хранения,
   ограниченные права оператора и автоматическая очистка; не хранить
   raw IP бессрочно и не публиковать его в аналитических labels.
5. Не смешивать с registration/referral attribution: origin кампании и
   security event имеют разные privacy, retry, retention и authority.

**Блокеры перед публичным включением:** additive PostgreSQL migration,
PG/HTTP/Chromium acceptance, два пользователя, два процесса, restart,
ложные IP/прокси, logout/revoke, Nginx, 1 vCPU RAM budget. Это
подзадачи в существующих A-03/A-09/I/H и не подтверждает deploy VPS.
