# NEwBOO -> TOR: защита от ботов, ловушки и ограничения действий (AB-01...AB-14)

**Статус 29.10.2026: исполнительский договор, НЕ внедрение кода.** Источник:
[NEwBOO main @ 17125ef758](https://github.com/HeisLuka/newboo/tree/17125ef758a791e9630505c14d24f7fe65aed228).
TOR проверен на main 767a1b351bb2357be599f10e8a8879683fc74c6d.

**Назначение:** снять недостаточную детализацию существующей F043 в
[functions-coverage-audit.tsv](functions-coverage-audit.tsv), A-09, C-10/C-11,
E-02/E-06/E-11, F-07/F-09/F-10/F-11, I-02/I-12/I-13 и RIL-04.
Это **подшаги уже существующих 117 задач**, не новый домен, не новые 14
основных задач и не разрешение менять 69 сценариев задним числом.

## Границы и действительные исходники

- [Antibot](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/antibot_service.py):
  known crawlers, trusted proxy, rate/fast reader navigation, progressive behavior-HMM
  decisions, SmartCaptcha, distributed penalty with bounded cache.
- [Honeypot service](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/honeypot_service.py),
  [mounted trap](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/honeypot.py),
  [auth web](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/web/routes/auth.py):
  hidden form fields and trap GET that returns empty 404/no-store/noindex. Donor
  suspect IP state is **process-local** with TTL; do not mislabel distributed.
- [Auth limiter](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/auth_rate_limit.py),
  [password spray](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/auth_spray_detection_service.py),
  [action limiter](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/action_rate_limit_service.py):
  account/IP and IP/subnet distinct identifier signals, scoped limits. Donor
  generic action limiter defaults to process-local state; direct book-share
  limits are also process-local. Distributed parity is a target, not legacy fact.
- [Blocklist](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/ip_blocklist_service.py),
  [unknown-route policy](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/unknown_route_policy_service.py),
  [load shedding](https://github.com/HeisLuka/newboo/blob/17125ef758a791e9630505c14d24f7fe65aed228/backend/app/services/load_shedding_policy_service.py).
  Legacy automatic IP block enforcement is not shown wired in production by
  [Notion moderation audit](https://app.notion.com/p/3cc32a84beec8174a17afb79bcc01905);
  it must not be copied as an already working guarantee.
- **Actual TOR partial parity:** [login_limiter.rs](../../crates/web/src/auth/login_limiter.rs)
  with durable PostgreSQL attempt counters and Nginx peer validation,
  [PgAccounts](../../crates/accounts/src/repositories/postgres.rs),
  [passive RIL](../../crates/web/src/ril.rs),
  [bounded behavior recorder](../../crates/traffic_behavior/src/services/recorder.rs).
  These are not a general anti-bot enforcement stack.

## Исполнительская матрица

| Подшаг | Владелец / задачи | Минимально проверяемое требование | Допуск |
| --- | --- | --- | --- |
| AB-01 — login/register counters | A-09 + I-05 | Сохранить действующий PostgreSQL atomic limiter по actor identity и доверенному peer-IP, нормальные 429 и Retry-After; лимит считать ДО Argon2. Нельзя объединять всех пользователей Nginx в один bucket. | P0, частично готово |
| AB-02 — password spraying | A-09 / AE-A04 | Отдельный detector по множеству разных учётных записей с одного проверенного IP/подсети: bounded distinct counts, soft friction, timed lock, общая state между двумя процессами, без user-enumeration и массовой блокировки NAT по одному событию. | P0 |
| AB-03 — hidden auth fields | A-09 / H-03 | Скрытые не-человеческие поля login/register; обработчик не создаёт account/session, фиксирует безопасный подозрительный факт, без возврата данных и без изменения нормального UI. | P1 |
| AB-04 — trap route | A-09 / I-02 | Явно зарегистрированный GET /__bp_honeypot/{name:path}; пустой 404, no-store, noindex, никакой открытой админки и раскрытия подозрительного состояния. Ограничить длину trap и storage. Сигнал НЕ даёт автоматический бессрочный IP ban. | P1 |
| AB-05 — progressive CAPTCHA | A-09 / H-03 / I | Условный challenge только по подтверждённому risk signal; Yandex SmartCaptcha или явно выключенный provider boundary, серверная verify и bounded timeout. Книги и бесплатные главы не требуют CAPTCHA по умолчанию. | P1, внешний provider |
| AB-06 — crawlers & trusted headers | A-09 / I-02 / F-08 | Известные поисковые crawlers не получают ложный block/SEO leakage; user-agent сам по себе не подтверждает личность. IP/Forwarded принимаются только от настроенного доверенного edge. Неподтверждённый X-Forwarded-For не создаёт новый bucket. | P0 |
| AB-07 — reader navigation | C-10/C-11 / A-09 | Частое перемещение между множеством глав диагностируется без утечки paid/version body. Платные версии остаются под D/C permission; долгий легальный сеанс не маркировать атакой. | P1 |
| AB-08 — scoped action limits | A-09 / E-02/E-06/E-11 / D/F | Отдельные лимиты на комментарии, массовую отправку книг, поддержку, рефералы, дорогие write API, платежные и рекламные действия. Нормативные caps sender/recipient нельзя смешивать с login caps. Сохранить CSRF, idempotency и domain ACL независимо от лимитера. | P0 для публичных записей |
| AB-09 — shared enforcement | A-09 / I-05 | Для многопроцессных write-операций выбран атомарный PostgreSQL counter/expiry или заведомо документированный более слабый диагностический режим. Restart и второй процесс не удваивают лимит. При отказе PG: явная per-scope fallback policy, а не silent unlimited. | P0 |
| AB-10 — manual IP policy | A-05/A-09 / ADM-02 / H-12 | Проверяемое admin ban/unban с ролью, CSRF, reason и audit; CIDR/IPv6 не обходят прокси; есть безопасный ручной rollback и правила false positives. | P1 |
| AB-11 — automatic containment | A-09 / I-13 | Автоматическое временное ограничение только по state transition bounded auth/spray detector, НЕ чтением тысячи security events на каждый login. Source donor auto-ban dormant; начало diagnostics-only. | P1, только после AB-02/10 |
| AB-12 — unknown route & overload | I-02/I-09/I-12 / RIL-04 | RIL предоставляет bounded taxonomy/route facts, а политика unknown/low-priority load shedding живёт вне RIL. Diagnostics -> audit -> enforce, known legitimate paths/SEO/readiness allowlisted. Сканер не может провоцировать дорогие DB lookup. | P1 |
| AB-13 — consumer hygiene | F-07/F-09/F-10/F-11/G-10 | Подозрительное открытие не накручивает рекомендацию, статистику, рекламный billable impression или реферальный reward. Истинные бизнес-факты покупки/возврата не стираются risk score. | P1 |
| AB-14 — release and observability | I-07/I-12/I-13 / A-09 | Типизированные active flags, fail-mode, отдельные bounded metrics 429/challenge/trap/spray/penalty, операторский disable, без raw IP/email/path/token. Нагрузочный budget на 1 vCPU/1 GiB, тесты Nginx, PostgreSQL, двух процессов и restart. | P0 before public traffic |

## Критические поведенческие инварианты

1. Login/register сохраняют PostgreSQL limiter, не открывают второй in-memory
   источник правды. В TOR сейчас 60/600 по проверенному peer IP и 8/600 по
   нормализованной учётной записи. Числа — текущие значения реализации,
   **не обещание копировать NEwBOO**; окончательно настроить по тестам нагрузки.
2. Honeypot — дополнительный сигнал. Не отдавать посетителю содержимое,
   не публиковать trap в sitemap и не превращать один GET в массовую блокировку
   NAT/CGNAT. В метриках только bounded trap family, без raw name/IP.
3. SmartCaptcha — по риску. Поисковый crawler и обычный читатель бесплатной
   главы не должны получать бесконечный challenge или 429 из-за чужой атаки.
4. Authentication, payment idempotency, paid entitlement, CSRF, body size и
   abuse rate — НЕ взаимозаменяемые решения. Отказ сервиса диагностики не
   предоставляет доступ к платному содержимому.
5. Инстансы Axum за Nginx видят client IP только от подтверждённой доверенной
   цепочки. Прямой обход edge, поддельные headers, IPv4/IPv6 и несколько
   значений X-Real-IP — отдельные отрицательные тесты.
6. RIL/Behavior Sketch — не enforcement. Consent, privacy, TTL, latency и
   queue-overflow не должны менять результат авторизации или открывать reader.

## Приёмка и свидетельства для PR

- Реальные PostgreSQL 18 и два соединения/процесса: 61-я попытка по одному
  verified IP и 9-я по одному account в текущем 600-секундном окне
  отвергаются с честным 429; другие независимые пользователи не блокируются.
  Параллельные попытки не обходят atomic counter; restart не сбрасывает окно.
- Недоверенные X-Forwarded-For/X-Real-IP не влияют на bucket; Nginx configured
  X-Real-IP проходит проверку; broken proxy config видна до выпуска.
- Скрытый field и trap дают безопасные реакции без session, с no-store/noindex;
  trap metadata не допускает high-cardinality Prometheus или unbounded memory.
- Spray на множестве login идентификаторов детектируется, но чужие неуспешные
  попытки к одному email не превращаются в безусловный отказ всем владельцам
  сети. Протестировать reset, срок наказания, false-positive восстановление.
- Перерасход scoped comment/share/support/paid API корректно ограничен:
  response 429 не меняет business state и не подменяет idempotency.
- Перегрузка не блокирует /readyz, уже купленную главу и админский rollback
  без явной проверенной политики. Проверить минимум 1 vCPU, 1 GiB RAM,
  PostgreSQL ограниченное число connections и bounded memory.
- В метриках нет email/IP/full URL/UA, в trace не пишутся секреты, security
  event retention определён. Fail-open/fail-closed решения для зависимостей
  зафиксированы отдельно; оператор видит причину 429 и риск ложного срабатывания.
- Финальный SHA: Repository rules + Rust fmt/Clippy + PG + HTTP/Chromium +
  негативные сценарии; dev-демо или успешная статическая классификация не
  означает реальной защиты рабочего VPS.

## Не переносить как готовую функциональность

- Donor honeypot suspect set in-memory/1h — не distributed reliable ban.
- Donor book-share process-local limits — не multi-process гарантия.
- Donor auto-IP enforcement — не доказан на production; проектировать
  bounded transitions, не event scans.
- Donor HMM / CAPTCHA необъявленные runtime flags могут быть неуправляемыми:
  [config audit](https://app.notion.com/p/3cd32a84beec81af96a4c7a72523807d).
- Donor URL crawler F021/S024 и interactive scenes S053-S056 заморожены;
  AB-12 не размораживает сканер или crawler.
- Эти документы не включают на VPS автоматическое IP-блокирование,
  CAPTCHA, WAF, Pingora или поисковую индексацию.

## Связанные дорожные карты

- [ACCESS-AUDIT-01…06: IP → account → session, GeoIP/risk и пользовательский контроль](account-access-audit-parity-v1.md). История входов **не равна** учёту rate-limit по IP, а геоаномалии требуют привязки к конкретной сессии.
- [61 functions and F043](functions-coverage-audit.tsv)
- [RIL-04 / A-09 dependency](../contracts/ril-platform-v1.md)
- [AE-A04 account spray](domains-a-e-parity-2026-10-26.md)
- [ADM-02 manual IP policy](admin-execution-contract.md)
- [Security route inventory](legacy-inventory/http-route-coverage.tsv)
- [Notion migration domain-gap register](https://app.notion.com/p/40432a84beec812e9182ee3246144dc5)
