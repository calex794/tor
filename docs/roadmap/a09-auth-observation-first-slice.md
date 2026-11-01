# A-09 / AB-02: auth observations (first diagnostic slice)

This changes neither authentication decisions nor existing 8/600 identity and
60/600 verified-IP limits. It never adds an automatic IP ban or exposes a
user-facing identity-enumeration API.

## Enablement / privacy

Observation is **off** unless \`TOR_AUTH_OBSERVATION_KEY\` contains a separately
generated 32-byte, non-zero secret encoded as 64 hexadecimal characters.
Provide the secret only through restricted host configuration; never commit it,
use the session/CSRF/RIL key, or print it in logs. Rotating this key breaks
cross-rotation pseudonym correlation. With no key, the existing aggregate
login-outcome metrics still run; no identified observation is collected.

Capture is bounded to eight concurrent inserts per Rust process. The login
response never waits for database writing. Excess inserts are dropped; this is
best-effort diagnostics, **not** a tamper-proof forensic audit. The next
integration slice must add a private dropped/disabled counter to operator
metrics and bounded batched ingestion if the load results warrant it.

Captured: completed password login result, UTC timestamp, HMAC target key,
optional user ID for known accounts, verified source and /24(v4) or /64(v6)
network HMAC, and browser family. No raw email, IP, password, password
fingerprint, cookie, full User-Agent, reset code or URL. Failed CSRF/forms have
no account-existence judgement. Unknown account status is private and never
affects HTTP response.

Retention: 7 days per detailed event, in batches of 1,000 every 5 minutes
via existing \`prune_security_data\`. Existing successful-session IP events
retain their distinct 30/90-day schedule.

## Operator queries (restricted SQL, no HTTP route)

\`\`\`sql
SELECT * FROM auth_observation_hourly ORDER BY hour_utc DESC LIMIT 48;
SELECT distinct_targets, attempts, successes, unknown_accounts
FROM auth_observation_sources_24h ORDER BY distinct_targets DESC LIMIT 20;
SELECT attempts, distinct_sources, successes, bad_passwords
FROM auth_observation_targets_24h ORDER BY attempts DESC LIMIT 20;
\`\`\`

The three views contain only pseudonymous keys or aggregate counts. Limit
database read access and do not publish raw rows, user IDs, or hashes on public
dashboards. With sparse traffic, unique source/target pseudonyms may still be
sensitive; only privileged operators should access these views.

## Acceptance

- Isolated PostgreSQL 18: migration applies, old login/session data survives,
  read-only views return expected hourly/source/target counts.
- Existing genuine HTTP login and CSRF behaviour unchanged; a known bad password,
  nonexistent email and valid login produce distinct private outcomes while
  publicly showing the same invalid-credentials message for both failures.
- Two application processes can write concurrently; restart preserves events.
  A failed insert cannot block login. Unknown/untrusted Nginx source yields null
  source key, not a global 127.0.0.1 group.
- No raw identifiers are logged or exposed. The site operates unchanged when
  observation is disabled. Load-test 1 vCPU/1 GiB under deliberate saturation;
  monitor dropped events, memory and p95 login latency.
- This PR stops at collection and SQL views. Deviations by hour-of-day, low-rate
  distributed spraying, incidents and automatic step-up require a separate
  acceptance slice and a safe false-positive review.

Never apply this migration to production until CI and the actual on-disk
migration ledger are inspected and the separate line-I operator authorizes
release of the exact tested main SHA.

## A-09 — операторский сценарий распыления (01.11.2026)

`cargo run -p tor-accounts --example auth_risk_report` читает
`TOR_AUTH_REPORT_DATABASE_URL` из закрытой конфигурации оператора.
Использовать отдельные права SELECT. Команда не запускается при входе читателя
и не публикует HTTP-маршрут; она выводит только тип и счётчики, без IP, email,
идентификаторов пользователей или псевдонимов. Ошибка подключения/таймаут
даёт ненулевой код завершения, а не ложное «угроз нет».

За последние 15 минут проверяются один источник → разные неудачные цели и
разные неудачные источники → одна цель. Минимум 15 попыток, 10 ошибочных
идентификаторов и 10 ошибок; неизвестный IP исключён. Успехи общего NAT
не увеличивают число ошибочных целей/источников. Максимум 50 групп,
транзакция только для чтения, statement_timeout=2s, lock_timeout=250ms,
work_mem=2MB. Старые и будущие события не входят в окно.

Испытания в существующем auth_observation_pg: две формы атаки, успешный общий
NAT, опечатки одного пользователя на занятом NAT, неизвестный IP, очистка;
дополнительно EXPLAIN ANALYZE той же SQL-строки на 100 000 старых и 1 000
недавних событий. Большая фикстура откатывается транзакцией.
Это синтетическая проверка PostgreSQL, не измерение домашнего железа.

Суточные адаптивные пороги, медленные многосуточные кампании, внешняя доставка
операторских предупреждений и автоматические ограничения не реализованы
этим диагностическим срезом. Полноту расширенной системы аномалий не заявлять.
