# TOR: production activation on PostgreSQL 18

**Decision, 26 October 2026:** run the actual Rust + PostgreSQL product
at https://pandaolate.ru. The zero-traffic site is not an excuse to keep an
in-memory demonstration. We retain only production essentials: least-privilege
roles, durable schema, real HTTPS, readiness, and a working rollback.

## Verified current VPS state

- PostgreSQL 18.6 running, 127.0.0.1:5432 only; 1 vCPU / 961 MiB RAM.
- DB tor owned by tor_migrator; tor_app / tor_migrator have connection
  limits 6 / 3 and neither is a superuser, createdb or createrole.
- /etc/tor/tor-app.env and tor-migrator.env are 0600 root:root.
- Last owner verification: **0 public domain tables**, still RAM demo
  in systemd; ~562 MiB MemAvailable. The server is not yet PG-backed.
- In GitHub TOR: nine canonical SQLx migrations, tor-migrate binary,
  PgCatalog, PgReader, PgAccounts, and the real PostgreSQL 18 CI suite.

## One-time switch to production

1. Open https://github.com/calex794/tor/actions/workflows/tor-vps-staging.yml
   and run it manually on main, with install=false. Confirm SUCCESS.
   This sends verified tor-web and tor-migrate executables to a SHA-named
   release folder on the VPS. No demo candidate is run and no active
   site or schema is changed by this stage.
2. Copy the exact **40-character main commit SHA** of that successful
   workflow into the operator commands (under root on the TOR VPS):

~~~bash
sha='<FULL_SHA_FROM_SUCCESSFUL_MAIN_STAGE>'
curl -fsSL "https://raw.githubusercontent.com/calex794/tor/$sha/ops/go-live-postgres-vps.sh" -o /root/tor-go-live.sh
bash -n /root/tor-go-live.sh &&
bash /root/tor-go-live.sh "$sha"
~~~

The one-shot operator script checks real PostgreSQL, SCRAM tor_app
credentials, both binary checksums, schema owner and the release location.
If the schema is empty, it calls the pinned canonical SQLx runner,
creates a local compressed backup first, and verifies nine donor SQLx versions plus any additive TOR versions
and at least 17 domain tables. It then installs a systemd override using
EnvironmentFile=/etc/tor/tor-app.env (read by systemd PID 1), sets
TOR_DEMO_MODE=0, TOR_AUTH_ENABLED=1 and
TOR_PUBLIC_ORIGIN=https://pandaolate.ru, and atomically switches the
versioned executable. Real database-backed readiness, registration/login
forms with CSRF, catalog, API and local Nginx HTTPS are checked.

**If production cutover fails**, the script restores the previous service
config and Rust binary. SQLx schema/data remain in PostgreSQL and are
not erased by binary rollback.

Acceptance markers:

~~~text
MIGRATION_HISTORY_ACCESS_OK
MIGRATIONS_OK: <N>/<N> SQLx versions, <TABLE_COUNT> domain tables
TOR_PRODUCTION_OK <SHA>
POSTGRES_READY_OK (<N>/<N> migrations, tor_app read-only schema history)
AUTH_PAGES_OK /register /login
HTTPS_READY_OK https://pandaolate.ru/readyz
REAL_DATA_MODE: PostgreSQL; no RAM demo
~~~

The catalog may be empty because no actual books have been entered yet.
**Do not insert fake production records.** Deliver authoring and real
content iteratively using the existing nine TOR workstreams.

**Production implementation note:** the cover image worker introduced
migration 202610260001 (four additional tables), so the release runner must
verify the embedded SQLx migration list rather than assuming exactly nine
total versions or 17 total tables. The CLI reports TOR_SQLX_APPLIED_OK N/N;
future additive migrations use the same mechanism.

## Controlled release after the first switch — 27 October 2026

**Current I-line safety decision:** merging into `main` runs tests but never invokes deployment or SSH. The former `workflow_run` trigger and `TOR_AUTO_DEPLOY` flag were removed from `.github/workflows/tor-vps-staging.yml`. The old auto-deploy instruction below is superseded.

To stage a candidate, run [tor VPS staged release](https://github.com/calex794/tor/actions/workflows/tor-vps-staging.yml) manually from `main` with `install=false` and an **empty** `confirm_sha`. The stage transfers only the SHA-pinned binaries; it does not switch the running service.

For `install=true`, supply `confirm_sha` equal to the **full 40-character `main` SHA of this workflow run**. The validation fails before checkout/build/SSH if the value differs, is truncated, or if a stage-only run supplied a confirmation. The existing root-verified SQLx schema and PG activation markers, healthy previous service, current-main recheck, rollback verification and HTTPS checks still apply. A missing migration marker prevents activation, even with correct confirmation.

The root-only go-live now also compares its pinned SHA with GitHub's exact
`refs/heads/main` **before migrating anything** and again before changing
the systemd service. This requires `python3` and access to
`https://api.github.com` on the VPS; verification fails closed if they are
missing or GitHub main has changed. If main changes *after* SQLx ran, the
old binary stays active but the added migrations persist: do not mistake
a refused switch for a database rollback. A fresh staging run is required
for the new main SHA.

The operator's version-pinned `go-live-postgres-vps.sh` remains the only procedure for applying new migrations. To advance an existing PostgreSQL schema, first perform backup and root-only migration/proof, then manually dispatch installation against the exact approved `main` SHA. Do not use `TOR_AUTO_DEPLOY=true`; it is ignored by the current workflow.

**Next engineering task:** integrate the migration upgrade process
for future schema version 010+ into the normal deploy path.
The SQLx runner recognizes all current additive migrations and verifies the
nine immutable historical donor versions. Do not silently deploy breaking DB changes:
binary rollback does not roll back DDL.

**Actual production limits:** accounts, catalog and public reading
are wired to PostgreSQL; full authoring, paid chapter delivery,
payment providers, ads, RIL and other domains require their own
shipped implementation and acceptance. Enable real off-server
backups with a restoration test before important user data.
The external URL book collector S024/F021 remains frozen.

## Schema-gated manual releases (updated 27 October 2026)

The previous automatic stage could activate a new binary without applying its
new SQLx migrations. In particular, B-06 requires migration
`202610260007_chapter_working_drafts.sql`; the new binary's `/readyz`
now fails closed if that migration or its working table is missing.

Staging always transfers the SHA-pinned binary and migrator. Before any
manually approved activation, the CI job compares the latest
SQLx version in the candidate's `migrations/` directory with the last version
recorded in `/var/lib/tor-release/schema-version` on the VPS. This marker is
written atomically by the **root-only**, version-pinned
`ops/go-live-postgres-vps.sh` after a successful migrator run and full
SQLx history/role verification. The marker's directory is root-owned and
not writable by the SSH deployment account.

The database marker alone is insufficient: it is written before the
application is switched and therefore could survive a failed first cutover.
The operator script now also writes the root-owned
`/var/lib/tor-release/pg-activated` marker **only after** the real
PostgreSQL application passes /readyz, login/registration and HTTPS checks.
Before manual activation, CI verifies this marker, the non-demo systemd
override and a healthy local /readyz. This prevents a successful schema
migration from accidentally authorizing a demo-mode deployment.

If the marker is absent or does not exactly match, the candidate can be
**staged but is never activated**. The operator applies migrations
through the standard go-live procedure using the same staged Git SHA; its
normal `/readyz`, HTTP and HTTPS checks still control the switch. A marker
is evidence of the verified migration version, not evidence that a specific
binary was activated. Only a logged `Active tor commit: <SHA>` or
`TOR_PRODUCTION_OK <SHA>` establishes deployment.

Manual installation also rechecks the exact current `main` SHA immediately before
the service switch. If another PR merged while compilation was running,
the obsolete candidate is rejected. This prevents a stale staged binary
from overwriting newer code.

Do not weaken the `tor_app` role to allow migrations, temporary tables, or
DDL. The migration runner uses `tor_migrator` under the operator's restricted
VPS procedure; no secrets are stored in GitHub releases or Notion.

## Подтверждение отката и восстановления после сбоя — 26 октября 2026

[Выпуск Actions №40382065303](https://github.com/calex794/tor/actions/runs/40382065303) доставил сборку `9f31dab2`, но после смены версии сервис не ответил на `127.0.0.1:3000/readyz`. Старый сценарий заменил ссылку `/opt/tor/current` обратно и попробовал перезапустить `tor-web.service`, однако сам перезапуск сообщил об ошибке. **Смена ссылки не доказывает восстановления сайта**. По одному журналу Actions нельзя утверждать, что прежняя версия работала после отказа.

Для следующих выпусков автоматический переход выполняется **только если прежняя версия уже активна и отвечает `ok` на `/readyz`**. После переключения новая версия должна пройти и `/readyz`, и открытие настоящей страницы регистрации с полем CSRF; положительный ответ демонстрационного сервера недостаточен. При отказе система возвращает предыдущую ссылку, делает повторный запуск и отдельно проверяет HTTP. В журнале различаются `ROLLBACK_VERIFIED` (сайт восстановлен) и `ROLLBACK_UNVERIFIED` (оператор должен вмешаться). Состояние systemd выводится через `systemctl show` без раскрытия секретов и полного журнала. Четыре испытания с подставными сервисами выполняются в CI без доступа к рабочему VPS.

**Действие оператора, только чтение**, перед новой попыткой (секреты/сырые строки журнала не публиковать):

~~~bash
readlink /opt/tor/current
systemctl show tor-web.service --no-pager -p ActiveState -p SubState -p Result -p ExecMainCode -p ExecMainStatus
curl -fsS --max-time 3 http://127.0.0.1:3000/readyz
~~~

Если ответ не `ok`, посмотреть `journalctl -u tor-web.service` **локально на VPS**, проверить текущую миграцию PostgreSQL и запуск из прошлой версии. Автоматическая выкладка не должна использоваться как способ чинить неизвестное нерабочее состояние. Применённые SQLx-миграции откатом программы не отменяются; для базы остаются отдельные резервная копия и проверка обратной совместимости.
